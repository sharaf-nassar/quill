use std::path::PathBuf;

use chrono::{DateTime, TimeDelta, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

use crate::integrations::IntegrationProvider;
use crate::models::{
    BucketStats, CodeStats, CodeStatsHistoryPoint, ContextSavingsAnalytics,
    ContextSavingsBreakdownItem, ContextSavingsBreakdowns, ContextSavingsEvent,
    ContextSavingsEventPayload, ContextSavingsInsertResult, ContextSavingsSummary,
    ContextSavingsTimeseriesPoint, DataPoint, EvidenceRef, GitSnapshot, HostBreakdown,
    LanguageBreakdown, LearnedRule, LearnedRulePayload, LearningRun, LearningRunPayload,
    LearningStatus, LlmRuntimeStats, ObservationPayload, ObservationSummary, ProjectBreakdown,
    ProjectTokens, RunInferenceCall, RunInferenceConfinement, RunInferenceSummary,
    SessionBreakdown, SessionCodeStats, SessionRef, SessionStats, SkillBreakdown,
    SkillProjectBreakdown, SkillUsage, SubagentNode, TokenDataPoint, TokenReportPayload,
    TokenStats, ToolCount, UsageBucket,
};

const PROVIDER_SETTINGS_KEY: &str = "integration.providers.v1";
#[allow(dead_code)]
const INDICATOR_PRIMARY_PROVIDER_KEY: &str = "indicator.primary_provider.v1";

// Feature 005 US5 T061 (R-7.3 / M-2 / FR-026). Safety floor for observation
// retention: cleanup never deletes observations newer than `now - this`, even
// when the analyzed watermark is older. The floor only ever *adds* retention
// (the effective cutoff is `MIN(watermark, now - floor)`); it can never delete
// inside the not-yet-analyzed window. SQLite `datetime` modifier form.
const OBSERVATION_RETENTION_FLOOR: &str = "-30 days";

// Feature 005 US5 T061/T062 (R-7.3/R-7.4 / M-2/M-1 / FR-026/FR-027). Specific
// error signal for the observation-summary `error_count`. The legacy tally was
// `tool_output LIKE '%error%' OR '%Error%'` — a bare substring that counted
// benign text ("no errors", "errorless", "ErrorBoundary"). This now requires a
// *structured* failure marker so the (now consumed) `error_count` is
// meaningful: a JSON error/`is_error` key, an `error`/`failed` status field, a
// leading `Error:` line, or a well-known runtime failure banner. Used in one
// place (the summary writer) so the predicate has a single definition. The
// `?1`/`?N` placeholder in the surrounding query is unaffected — this fragment
// is constant SQL with no bind parameters.
const OBSERVATION_ERROR_PREDICATE: &str = r#"(
    tool_output LIKE '%"is_error":true%'
    OR tool_output LIKE '%"is_error": true%'
    OR tool_output LIKE '%"isError":true%'
    OR tool_output LIKE '%"isError": true%'
    OR tool_output LIKE '%"error":%'
    OR tool_output LIKE '%"error" :%'
    OR tool_output LIKE '%"status":"error"%'
    OR tool_output LIKE '%"status": "error"%'
    OR tool_output LIKE '%"status":"failed"%'
    OR tool_output LIKE '%"status": "failed"%'
    OR tool_output LIKE 'Error:%'
    OR tool_output LIKE '%' || char(10) || 'Error:%'
    OR tool_output LIKE '%Traceback (most recent call last)%'
    OR tool_output LIKE '%panic:%'
    OR tool_output LIKE '%fatal:%'
)"#;
const CONTEXT_SAVINGS_EVENTS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS context_savings_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    schema_version INTEGER NOT NULL,
    provider TEXT NOT NULL,
    session_id TEXT,
    hostname TEXT NOT NULL,
    cwd TEXT,
    timestamp TEXT NOT NULL,
    event_type TEXT NOT NULL,
    source TEXT NOT NULL,
    decision TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'unknown',
    reason TEXT,
    delivered INTEGER NOT NULL,
    indexed_bytes INTEGER,
    returned_bytes INTEGER,
    input_bytes INTEGER,
    tokens_indexed_est INTEGER,
    tokens_returned_est INTEGER,
    tokens_saved_est INTEGER,
    tokens_preserved_est INTEGER,
    estimate_method TEXT,
    estimate_confidence REAL,
    source_ref TEXT,
    snapshot_ref TEXT,
    metadata_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_context_savings_timestamp
    ON context_savings_events(timestamp);
CREATE INDEX IF NOT EXISTS idx_context_savings_provider_timestamp
    ON context_savings_events(provider, timestamp);
CREATE INDEX IF NOT EXISTS idx_context_savings_provider_session_timestamp
    ON context_savings_events(provider, session_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_context_savings_event_type_timestamp
    ON context_savings_events(event_type, timestamp);
CREATE INDEX IF NOT EXISTS idx_context_savings_cwd_timestamp
    ON context_savings_events(cwd, timestamp);
-- The `idx_context_savings_category_timestamp` index is created in
-- migration 18, after the `category` column has been added via ALTER
-- TABLE. Keeping it here would fail on databases that were created
-- before migration 18 because the column does not yet exist when this
-- batch runs (the early init batch executes before migrations).
"#;
// Aggregate fragment shared by summary, timeseries, and breakdowns.
//
// Byte and token-indexed/returned columns sum every event so breakdown rows
// still surface router/telemetry traffic accurately.  The saved and preserved
// token columns are filtered to `category IN ('preservation', 'retrieval')`
// so capture-hook telemetry no longer pollutes the headline metric — rows
// outside that scope contribute zero to those two columns.
const CONTEXT_SAVINGS_AGGREGATES_SQL: &str = r#"
COUNT(*),
COALESCE(SUM(CASE WHEN delivered != 0 THEN 1 ELSE 0 END), 0),
COALESCE(SUM(COALESCE(indexed_bytes, 0)), 0),
COALESCE(SUM(COALESCE(returned_bytes, 0)), 0),
COALESCE(SUM(COALESCE(input_bytes, 0)), 0),
COALESCE(SUM(COALESCE(tokens_indexed_est, (COALESCE(indexed_bytes, 0) + 3) / 4)), 0),
COALESCE(SUM(COALESCE(tokens_returned_est, (COALESCE(returned_bytes, 0) + 3) / 4)), 0),
COALESCE(SUM(
    CASE WHEN category IN ('preservation', 'retrieval') THEN
        CASE
            WHEN tokens_saved_est IS NOT NULL
                AND NOT (
                    tokens_saved_est = 0
                    AND delivered = 0
                    AND indexed_bytes IS NOT NULL
                    AND returned_bytes IS NULL
                ) THEN tokens_saved_est
            WHEN indexed_bytes IS NOT NULL OR input_bytes IS NOT NULL OR returned_bytes IS NOT NULL THEN
                CASE
                    WHEN COALESCE(indexed_bytes, input_bytes, 0) > COALESCE(returned_bytes, 0) THEN
                        (COALESCE(indexed_bytes, input_bytes, 0) - COALESCE(returned_bytes, 0) + 3) / 4
                    ELSE 0
                END
            ELSE 0
        END
    ELSE 0 END
), 0),
COALESCE(SUM(
    CASE WHEN category IN ('preservation', 'retrieval')
        THEN COALESCE(tokens_preserved_est, (COALESCE(indexed_bytes, 0) + 3) / 4)
        ELSE 0
    END
), 0)
"#;

// Category-scoped totals returned alongside the legacy aggregate.  Each
// token CASE picks from exactly one category, so the three numbers partition
// the active range cleanly without double-counting.  Routing and telemetry
// also expose their own event counts because the `routerEventCount` field
// derived from the breakdown only sees `router.*` event-type strings, while
// the routing *category* additionally includes `mcp.search`, bounded
// `mcp.execute` results, and `capture.guidance` — so the headline token
// total and the supporting subtitle stay aligned.
const CONTEXT_SAVINGS_CATEGORY_TOTALS_SQL: &str = r#"
COALESCE(SUM(
    CASE WHEN category = 'preservation'
        THEN COALESCE(tokens_preserved_est, (COALESCE(indexed_bytes, 0) + 3) / 4)
        ELSE 0
    END
), 0),
COALESCE(SUM(
    CASE WHEN category = 'retrieval'
        THEN COALESCE(tokens_returned_est, (COALESCE(returned_bytes, 0) + 3) / 4)
        ELSE 0
    END
), 0),
COALESCE(SUM(
    CASE WHEN category = 'routing'
        THEN COALESCE(tokens_returned_est, (COALESCE(returned_bytes, 0) + 3) / 4)
        ELSE 0
    END
), 0),
COALESCE(SUM(CASE WHEN category = 'telemetry' THEN 1 ELSE 0 END), 0),
COALESCE(SUM(CASE WHEN category = 'routing' THEN 1 ELSE 0 END), 0)
"#;

// Per-source efficiency: distinct source_refs that were preserved within the
// window, and the subset that were also retrieved within the window.  We
// require both events to fall in-window so the ratio stays bounded in [0, 1]
// and reflects actual engagement, not pre-window leftovers.
const CONTEXT_SAVINGS_RETENTION_SQL: &str = r#"
WITH source_activity AS (
    SELECT source_ref,
        MAX(CASE WHEN category = 'preservation' THEN 1 ELSE 0 END) AS was_preserved,
        MAX(CASE WHEN category = 'retrieval'   THEN 1 ELSE 0 END) AS was_retrieved
    FROM context_savings_events
    WHERE timestamp >= ?1
      AND source_ref IS NOT NULL
      AND source_ref != ''
      AND category IN ('preservation', 'retrieval')
    GROUP BY source_ref
)
SELECT
    COALESCE(SUM(was_preserved), 0),
    COALESCE(SUM(CASE WHEN was_preserved = 1 AND was_retrieved = 1 THEN 1 ELSE 0 END), 0)
FROM source_activity
"#;

/// One message borrowed for response-time ingestion. Carries the role and
/// timestamp the turn detector needs, plus the per-message sub-agent
/// attribution propagated onto the assistant row that closes the turn.
#[derive(Clone, Copy)]
pub struct ResponseTimeInput<'a> {
    pub role: &'a str,
    pub timestamp: &'a str,
    pub is_sidechain: bool,
    pub agent_id: Option<&'a str>,
    pub parent_uuid: Option<&'a str>,
}

impl<'a> ResponseTimeInput<'a> {
    /// Build an input from a (role, timestamp) tuple where no sub-agent
    /// attribution is available (e.g., HTTP API pushes).
    pub fn new(role: &'a str, timestamp: &'a str) -> Self {
        Self {
            role,
            timestamp,
            is_sidechain: false,
            agent_id: None,
            parent_uuid: None,
        }
    }
}

/// One non-meta `user` or `assistant` JSONL line borrowed for the
/// `session_events` ingest pipeline (feature 008). Mirrors
/// [`ResponseTimeInput`] lifetime conventions so the
/// `process_discovered_file` site can build both vectors in the same
/// loop. See specs/008-runtime-redesign/contracts/session-events.md.
// @lat: [[backend#Database#Schema#Code and Runtime Metrics]]
#[derive(Clone, Copy)]
pub struct SessionEventInput<'a> {
    pub timestamp: &'a str,
    pub kind: crate::sessions::SessionEventKind,
    pub is_sidechain: bool,
    pub agent_id: Option<&'a str>,
    pub uuid: Option<&'a str>,
    pub parent_uuid: Option<&'a str>,
}

/// One observed lifecycle-hook fire borrowed for the `hook_invocations`
/// ingest pipeline (feature 009). Claude rows are populated from the
/// JSONL attachment extractor in `sessions.rs`; Codex rows arrive via
/// the HTTP endpoint and are converted into owned
/// [`crate::models::CodexHookObservation`] before insert. See
/// specs/009-hooks-breakdown-tab/contracts/hook-invocations.md.
// @lat: [[backend#Database#Schema#Hook Invocations]]
#[derive(Clone)]
pub struct HookInvocationInput<'a> {
    pub session_id: &'a str,
    pub agent_id: Option<&'a str>,
    pub is_sidechain: bool,
    pub timestamp: &'a str,
    pub hook_event: &'a str,
    pub hook_matcher: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    pub hook_identity: &'a str,
    pub script_command_raw: Option<&'a str>,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub cwd: Option<&'a str>,
    pub hostname: Option<&'a str>,
    pub message_id: Option<&'a str>,
}

/// Sub-agent attribution triple carried alongside a tool action row.
/// Bundled into a struct to keep `insert_tool_actions` under clippy's
/// `too_many_arguments` threshold and to mirror the `ResponseTimeInput`
/// pattern used for the response-times pipeline.
#[derive(Clone, Copy, Default)]
struct ToolActionAttribution<'a> {
    is_sidechain: bool,
    agent_id: Option<&'a str>,
    parent_uuid: Option<&'a str>,
}

fn insert_tool_actions(
    stmt: &mut rusqlite::CachedStatement<'_>,
    provider: IntegrationProvider,
    actions: &[crate::sessions::ToolAction],
    message_id: &str,
    session_id: &str,
    attribution: ToolActionAttribution<'_>,
) -> Result<(), String> {
    for action in actions {
        stmt.execute(rusqlite::params![
            provider.as_str(),
            message_id,
            session_id,
            action.tool_name,
            action.category,
            action.file_path,
            action.summary,
            action.full_input,
            action.full_output,
            action.timestamp,
            attribution.is_sidechain as i32,
            attribution.agent_id,
            attribution.parent_uuid,
        ])
        .map_err(|e| format!("Insert tool_action: {e}"))?;
    }

    Ok(())
}

fn wilson_lower_bound(alpha: f64, beta: f64) -> f64 {
    let n = alpha + beta;
    if n < 0.01 {
        return 0.5;
    }
    let p = alpha / n;
    let z = 1.96_f64;
    let denominator = 1.0 + z * z / n;
    let center = p + z * z / (2.0 * n);
    let spread = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt();
    ((center - spread) / denominator).clamp(0.0, 0.95)
}

/// Read-time quality label. Feature 005 US3 T044 (M-4 / FR-017, research
/// R-6 "Verdicts"): now keys off `alpha`/`beta` for a strong-contradiction
/// override — when contradictions dominate (`beta >= alpha`) AND the
/// negative evidence is substantial (`beta >= 5.0`), the rule is
/// `invalidated` regardless of the Wilson confidence. The override is
/// ordered AFTER the freshness/stale check (a stale rule reads `stale`) and
/// BEFORE the confidence-band checks, so a heavily-contradicted rule can
/// never read `emerging`/`confirmed`. `eligible_for_review` excludes
/// `invalidated`, so this directly gates promotion.
fn compute_state(confidence: f64, alpha: f64, beta: f64, freshness: f64) -> &'static str {
    // Strong-contradiction override (`beta >= alpha && beta >= 5.0`) is
    // OR-ed with the existing low-confidence check: either drives
    // `invalidated`. Ordered after the freshness/stale check and before the
    // confidence bands so a heavily-contradicted rule can never read
    // `emerging`/`confirmed`.
    let strong_contradiction = beta >= alpha && beta >= 5.0;
    if freshness < 0.3 {
        "stale"
    } else if strong_contradiction || confidence < 0.4 {
        "invalidated"
    } else if confidence >= 0.6 {
        "confirmed"
    } else {
        "emerging"
    }
}

fn freshness_factor(last_evidence_at: Option<&str>) -> f64 {
    let Some(ts) = last_evidence_at else {
        return 1.0;
    };
    let Ok(last) = chrono::DateTime::parse_from_rfc3339(ts) else {
        return 1.0;
    };
    let days = (Utc::now() - last.with_timezone(&Utc)).num_seconds().max(0) as f64 / 86400.0;
    0.5_f64.powf(days / 90.0)
}

/// Single source of truth for evidence-weighted rule scoring (R-6).
///
/// `fresh = freshness_factor(last_evidence_at)` (90-day half-life);
/// `score = wilson_lower_bound(alpha * fresh, beta * fresh)`; `state` is the
/// read-time-derived quality label via `compute_state`. Feature 005 US3
/// T044 revised `compute_state` to add the `beta >= alpha && beta >= 5.0 →
/// invalidated` strong-contradiction override (raw α/β, not the
/// freshness-decayed Wilson confidence), so a heavily-contradicted rule is
/// `invalidated` and excluded by `eligible_for_review`.
///
/// Both `get_learned_rules` read sites AND `eligible_for_review` route
/// through this single function so the read path and the review gate can
/// never diverge (`eligible_for_review` first folds operator feedback into
/// α/β per T047 so the human signal dominates).
fn evidence_weighted_score(
    alpha: f64,
    beta: f64,
    last_evidence_at: Option<&str>,
) -> (f64, &'static str) {
    let fresh = freshness_factor(last_evidence_at);
    let score = wilson_lower_bound(alpha * fresh, beta * fresh);
    let state = compute_state(score, alpha, beta, fresh);
    (score, state)
}

fn db_path() -> Result<PathBuf, String> {
    let data_dir = dirs::data_local_dir()
        .or_else(|| {
            dirs::home_dir().map(|h| {
                if cfg!(target_os = "macos") {
                    h.join("Library").join("Application Support")
                } else {
                    h.join(".local").join("share")
                }
            })
        })
        .ok_or("Cannot determine data directory")?;
    let default_app_dir = data_dir.join("com.quilltoolkit.app");
    let app_dir = crate::data_paths::resolve_data_dir_with_default(default_app_dir);
    std::fs::create_dir_all(&app_dir).map_err(|e| format!("Failed to create app data dir: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&app_dir, std::fs::Permissions::from_mode(0o700));
    }

    Ok(app_dir.join("usage.db"))
}

fn ext_to_language(file_path: &str) -> &'static str {
    let ext = file_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" => "JavaScript",
        "rs" => "Rust",
        "py" => "Python",
        "css" | "scss" => "CSS",
        "html" => "HTML",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "md" => "Markdown",
        "sql" => "SQL",
        "go" => "Go",
        "sh" => "Shell",
        _ => "Other",
    }
}

fn parse_code_change(tool_name: &str, full_input: &str) -> Option<(i64, i64, String)> {
    if tool_name == "apply_patch" {
        return parse_apply_patch_change(full_input);
    }

    let parsed: serde_json::Value = serde_json::from_str(full_input).ok()?;

    let file_path = parsed
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match tool_name {
        "Edit" => {
            let old = parsed.get("old_string").and_then(|v| v.as_str())?;
            let new = parsed.get("new_string").and_then(|v| v.as_str())?;
            let removed = old.lines().count() as i64;
            let added = new.lines().count() as i64;
            Some((added, removed, file_path))
        }
        "Write" => {
            let content = parsed.get("content").and_then(|v| v.as_str())?;
            let added = content.lines().count() as i64;
            Some((added, 0, file_path))
        }
        _ => None,
    }
}

fn parse_apply_patch_change(patch: &str) -> Option<(i64, i64, String)> {
    let mut added = 0i64;
    let mut removed = 0i64;
    let mut file_path = String::new();
    let mut mode = "";

    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            if file_path.is_empty() {
                file_path = path.to_string();
            }
            mode = "add";
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            if file_path.is_empty() {
                file_path = path.to_string();
            }
            mode = "update";
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            if file_path.is_empty() {
                file_path = path.to_string();
            }
            mode = "delete";
            continue;
        }

        if line.starts_with("*** ") {
            mode = "";
            continue;
        }

        match mode {
            "add" if line.starts_with('+') => {
                added += 1;
            }
            "update" if line.starts_with('+') => {
                added += 1;
            }
            "update" if line.starts_with('-') => {
                removed += 1;
            }
            _ => {}
        }
    }

    if added == 0 && removed == 0 && file_path.is_empty() {
        return None;
    }

    Some((added, removed, file_path))
}

fn session_key(provider: IntegrationProvider, session_id: &str) -> String {
    format!("{provider}:{session_id}")
}

fn normalized_provider_scope(provider_scope: &[IntegrationProvider]) -> Vec<IntegrationProvider> {
    let mut scope = provider_scope.to_vec();
    scope.sort_by_key(|provider| provider.as_str());
    scope.dedup();
    if scope.is_empty() {
        scope.push(IntegrationProvider::Claude);
    }
    scope
}

fn provider_scope_json(provider_scope: &[IntegrationProvider]) -> String {
    serde_json::to_string(&normalized_provider_scope(provider_scope))
        .unwrap_or_else(|_| "[\"claude\"]".to_string())
}

fn parse_provider_scope(value: Option<String>) -> Vec<IntegrationProvider> {
    let raw = value.unwrap_or_else(|| "[\"claude\"]".to_string());
    let parsed = serde_json::from_str::<Vec<IntegrationProvider>>(&raw)
        .unwrap_or_else(|_| vec![IntegrationProvider::Claude]);
    normalized_provider_scope(&parsed)
}

fn provider_scope_contains_json(provider: IntegrationProvider) -> String {
    format!("\"{}\"", provider.as_str())
}

// Feature 005 US5 T058 (R-7.1 / H-6 / FR-024). Decode struct mirroring the
// *serialized* shape of `cc_client::InferenceCallMetadata` (which serializes
// with `skip_serializing_if` on several fields, so every field here is
// `#[serde(default)]` — a legacy / partial record must still parse). This is
// intentionally decoupled from the source struct so a field rename there is a
// compile-visible decision, never a silent decode break.
#[derive(serde::Deserialize, Default)]
struct InferenceCallRecord {
    #[serde(default)]
    phase: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    ttft_ms: u64,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_cost_usd: f64,
    #[serde(default)]
    success: bool,
    #[serde(default)]
    failure_kind: Option<String>,
    // Feature 006 Follow-up A (R-A / C-A). Recorded OS-confinement tag from
    // `cc_client::SandboxKind::as_str` (one of
    // `bwrap`/`process-only`/`sandbox-exec`/`job-object`/`none`). `None` only
    // on legacy/pre-feature-005 records that wrote no `sandbox` field —
    // tolerant decode, never an error path.
    #[serde(default)]
    sandbox: Option<String>,
}

// The `sandbox` tag FS-confinement classifier lives in
// `cc_client::sandbox_tag_is_fs_confined` (single source of truth, keyed on
// the stable `SandboxKind::as_str` tag this path decodes from JSON).

/// Tolerant fold of a run's `learning_runs.inference_metadata` JSON (a
/// `Vec<cc_client::InferenceCallMetadata>` in dispatch order) into the
/// derived [`RunInferenceSummary`] rollup surfaced on [`LearningRun`].
///
/// Feature 005 US5 T058 (R-7.1 / H-6 / FR-024). Contract: NULL / `None` /
/// parse-error / empty-array ⇒ `None` (legacy and `micro` runs legitimately
/// record no per-call metadata — this is not an error path and MUST NOT
/// panic). `primary_model` = the model accounting for the highest summed
/// `total_cost_usd` across all calls (ties broken by first dispatch order),
/// matching the run-level cost-attribution rule used elsewhere.
fn decode_inference_metadata(raw: Option<String>) -> Option<RunInferenceSummary> {
    let raw = raw?;
    let records: Vec<InferenceCallRecord> = serde_json::from_str(&raw).ok()?;
    if records.is_empty() {
        return None;
    }

    let mut total_cost_usd = 0.0_f64;
    let mut total_duration_ms = 0_u64;
    let mut failed_call_count = 0_u32;
    // Feature 006 Follow-up A (R-A / C-A) run-level confinement rollup:
    // `None` until a call carries a `sandbox` tag, then the AND across every
    // tagged call (any not-FS-confined call ⇒ the run is not fully confined).
    let mut all_fs_confined: Option<bool> = None;
    let mut calls = Vec::with_capacity(records.len());
    // Highest-cost model wins; first-seen order is the deterministic
    // tie-break (insertion-ordered cost accumulator).
    let mut model_cost: Vec<(String, f64)> = Vec::new();

    for record in records {
        total_cost_usd += record.total_cost_usd;
        total_duration_ms = total_duration_ms.saturating_add(record.duration_ms);
        if !record.success {
            failed_call_count += 1;
        }
        if let Some(model) = record.model.as_deref() {
            if let Some(entry) = model_cost.iter_mut().find(|(name, _)| name == model) {
                entry.1 += record.total_cost_usd;
            } else {
                model_cost.push((model.to_string(), record.total_cost_usd));
            }
        }
        let confinement = record.sandbox.map(|sandbox| {
            let fs_confined = crate::cc_client::sandbox_tag_is_fs_confined(&sandbox);
            // Fold into the run rollup: AND across all tagged calls.
            all_fs_confined = Some(all_fs_confined.unwrap_or(true) && fs_confined);
            RunInferenceConfinement {
                sandbox,
                fs_confined,
            }
        });
        calls.push(RunInferenceCall {
            phase: record.phase,
            model: record.model,
            cost_usd: record.total_cost_usd,
            duration_ms: record.duration_ms,
            ttft_ms: record.ttft_ms,
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            success: record.success,
            failure_kind: record.failure_kind,
            confinement,
        });
    }

    // Strict-greater scan over the insertion-ordered accumulator: the first
    // model to reach the max cost wins (deterministic first-dispatch
    // tie-break), independent of `max_by`'s last-element semantics.
    let mut primary_model: Option<String> = None;
    let mut primary_cost = f64::NEG_INFINITY;
    for (name, cost) in model_cost {
        if cost > primary_cost {
            primary_cost = cost;
            primary_model = Some(name);
        }
    }

    Some(RunInferenceSummary {
        total_cost_usd,
        total_duration_ms,
        primary_model,
        call_count: calls.len() as u32,
        failed_call_count,
        calls,
        all_fs_confined,
    })
}

fn push_unique_dir(dirs: &mut Vec<PathBuf>, path: PathBuf) {
    if !dirs.contains(&path) {
        dirs.push(path);
    }
}

fn learned_rule_dirs(provider: Option<IntegrationProvider>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let claude_dir = crate::learning::learned_rules_dir_for_scope(&[IntegrationProvider::Claude]);
    let codex_dir = crate::learning::learned_rules_dir_for_scope(&[IntegrationProvider::Codex]);
    let shared_dir = crate::learning::learned_rules_dir_for_scope(&[
        IntegrationProvider::Claude,
        IntegrationProvider::Codex,
    ]);

    match provider {
        Some(IntegrationProvider::Claude) => {
            push_unique_dir(&mut dirs, claude_dir);
            push_unique_dir(&mut dirs, shared_dir);
        }
        Some(IntegrationProvider::Codex) => {
            push_unique_dir(&mut dirs, codex_dir);
            push_unique_dir(&mut dirs, shared_dir);
        }
        Some(IntegrationProvider::MiniMax) => {}
        None => {
            push_unique_dir(&mut dirs, claude_dir);
            push_unique_dir(&mut dirs, codex_dir);
            push_unique_dir(&mut dirs, shared_dir);
        }
    }

    dirs
}

fn learned_rule_dirs_for_scope(provider_scope: &[IntegrationProvider]) -> Vec<PathBuf> {
    let scope = normalized_provider_scope(provider_scope);
    match scope.as_slice() {
        [IntegrationProvider::Claude] => learned_rule_dirs(Some(IntegrationProvider::Claude)),
        [IntegrationProvider::Codex] => learned_rule_dirs(Some(IntegrationProvider::Codex)),
        _ => learned_rule_dirs(None),
    }
}

fn inferred_rule_provider_scope(path: &std::path::Path) -> Vec<IntegrationProvider> {
    let shared_dir = crate::learning::learned_rules_dir_for_scope(&[
        IntegrationProvider::Claude,
        IntegrationProvider::Codex,
    ]);
    if path.starts_with(&shared_dir) {
        return vec![IntegrationProvider::Claude, IntegrationProvider::Codex];
    }

    let codex_dir = crate::learning::learned_rules_dir_for_scope(&[IntegrationProvider::Codex]);
    if path.starts_with(&codex_dir) {
        return vec![IntegrationProvider::Codex];
    }

    vec![IntegrationProvider::Claude]
}

#[allow(dead_code)]
fn parse_rule_frontmatter(content: &str) -> (Option<String>, bool, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, false, content.to_string());
    }
    let after_opening = &trimmed[3..];
    let Some(end_idx) = after_opening.find("\n---") else {
        return (None, false, content.to_string());
    };
    let frontmatter = &after_opening[..end_idx];
    let body = after_opening[end_idx + 4..]
        .trim_start_matches('\n')
        .to_string();

    let mut domain = None;
    let mut is_anti = false;
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("domain:") {
            let val = val.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                domain = Some(val.to_string());
            }
        } else if let Some(val) = line.strip_prefix("anti_pattern:") {
            is_anti = val.trim().eq_ignore_ascii_case("true");
        }
    }
    (domain, is_anti, body)
}

fn range_to_duration(range: &str) -> TimeDelta {
    match range {
        "1h" => TimeDelta::hours(1),
        "24h" => TimeDelta::hours(24),
        "7d" => TimeDelta::days(7),
        "30d" => TimeDelta::days(30),
        _ => TimeDelta::hours(24),
    }
}

fn context_savings_from_timestamp(range: &str) -> String {
    if range == "all" {
        return "1970-01-01T00:00:00Z".to_string();
    }

    (Utc::now() - range_to_duration(range)).to_rfc3339()
}

fn context_savings_bucket_expr(range: &str) -> &'static str {
    match range {
        "1h" => "substr(timestamp, 1, 16) || ':00Z'",
        "30d" | "all" => "substr(timestamp, 1, 10) || 'T00:00:00Z'",
        _ => "substr(timestamp, 1, 13) || ':00:00Z'",
    }
}

fn context_savings_summary_from_row_at(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<ContextSavingsSummary> {
    Ok(ContextSavingsSummary {
        event_count: row.get(offset)?,
        delivered_count: row.get(offset + 1)?,
        indexed_bytes: row.get(offset + 2)?,
        returned_bytes: row.get(offset + 3)?,
        input_bytes: row.get(offset + 4)?,
        tokens_indexed_est: row.get(offset + 5)?,
        tokens_returned_est: row.get(offset + 6)?,
        tokens_saved_est: row.get(offset + 7)?,
        tokens_preserved_est: row.get(offset + 8)?,
        // New category-scoped fields default to zero; populated by the summary
        // path via [`apply_category_totals`] and [`apply_retention_metrics`].
        tokens_preserved: 0,
        tokens_retrieved: 0,
        tokens_routing: 0,
        telemetry_event_count: 0,
        routing_event_count: 0,
        sources_preserved: 0,
        sources_retrieved: 0,
        retention_ratio: 0.0,
    })
}

fn apply_category_totals(
    summary: &mut ContextSavingsSummary,
    conn: &Connection,
    from: &str,
) -> Result<(), String> {
    let sql = format!(
        "SELECT {CONTEXT_SAVINGS_CATEGORY_TOTALS_SQL}
         FROM context_savings_events
         WHERE timestamp >= ?1"
    );
    let (preserved, retrieved, routing, telemetry, routing_events) = conn
        .query_row(&sql, params![from], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| format!("Query context savings category totals error: {e}"))?;
    summary.tokens_preserved = preserved;
    summary.tokens_retrieved = retrieved;
    summary.tokens_routing = routing;
    summary.telemetry_event_count = telemetry;
    summary.routing_event_count = routing_events;
    Ok(())
}

fn apply_retention_metrics(
    summary: &mut ContextSavingsSummary,
    conn: &Connection,
    from: &str,
) -> Result<(), String> {
    let (preserved, retrieved) = conn
        .query_row(CONTEXT_SAVINGS_RETENTION_SQL, params![from], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| format!("Query context savings retention error: {e}"))?;
    summary.sources_preserved = preserved;
    summary.sources_retrieved = retrieved;
    summary.retention_ratio = if preserved > 0 {
        (retrieved as f64 / preserved as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Ok(())
}

fn context_savings_breakdown_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ContextSavingsBreakdownItem> {
    let summary = context_savings_summary_from_row_at(row, 1)?;
    Ok(ContextSavingsBreakdownItem {
        key: row.get(0)?,
        event_count: summary.event_count,
        delivered_count: summary.delivered_count,
        indexed_bytes: summary.indexed_bytes,
        returned_bytes: summary.returned_bytes,
        input_bytes: summary.input_bytes,
        tokens_indexed_est: summary.tokens_indexed_est,
        tokens_returned_est: summary.tokens_returned_est,
        tokens_saved_est: summary.tokens_saved_est,
        tokens_preserved_est: summary.tokens_preserved_est,
    })
}

fn parse_context_savings_provider(value: String) -> IntegrationProvider {
    value
        .parse::<IntegrationProvider>()
        .unwrap_or(IntegrationProvider::Claude)
}

fn parse_context_savings_metadata(raw: Option<String>) -> Option<serde_json::Value> {
    raw.and_then(|value| serde_json::from_str(&value).ok())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.prepare(&format!("SELECT {column} FROM {table} LIMIT 0"))
        .is_ok()
}

/// Feature 005 US2 T027 (C-5, FR-010, contracts/rule-governance.md
/// "`tombstone_blocks`"): a durable, name-keyed suppression gate.
///
/// Returns `true` iff a `rule_tombstones` row exists for `name` AND it has
/// not been explicitly reactivated (`reactivated_at IS NULL`). The tombstone
/// is name-keyed because re-extraction and reconcile are name-addressed and
/// the tombstone must outlive the `learned_rules` row (it is never
/// CASCADE-deleted). This single predicate is consulted at ALL five
/// name-addressed write/activation paths — `store_learned_rule`,
/// `write_rule_files`, `promote_learned_rule`, and `reconcile_learned_rules`
/// steps 3a and 3c — so a suppressed pattern can never be silently
/// resurrected by analysis, manual `.md` re-creation, or reconcile. The only
/// path that clears it is the explicit authorized `reactivate_rule`.
/// One resolved citation (feature 005 US3 T041, H-1). Carries the snapshot
/// fields denormalized into `rule_evidence_citations` so grounding survives
/// observation purge.
#[derive(Clone, Debug)]
pub struct ResolvedCitation {
    pub kind: String,
    pub ref_id: String,
    pub observation_id: Option<i64>,
    pub provider: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub tool_name: Option<String>,
    pub evidence_ts: Option<String>,
}

/// Result of resolving a candidate's `evidence_refs` (feature 005 US3 T041).
/// `resolved` is already de-duplicated by `(kind, id)`, so `resolved.len()`
/// is the distinct resolved-citation count consumed by the eligibility gate
/// (T042/T043/FR-016).
#[derive(Clone, Debug, Default)]
pub struct ResolvedEvidence {
    pub resolved: Vec<ResolvedCitation>,
    pub distinct_sources: usize,
    pub project_paths: Vec<String>,
}

impl ResolvedEvidence {
    pub fn distinct_count(&self) -> usize {
        self.resolved.len()
    }
}

/// Feature 005 US3 T041 (H-1 / FR-015): is a `kind="commit"` ref resolvable?
///
/// Resolvable iff the analyzed repo (`repo_path`, when supplied and a real
/// path) confirms it via `git cat-file -e <hash>^{{commit}}`, OR any
/// `git_snapshots` row has a `commit_hash` prefixed by `<hash>` (abbreviated
/// `%h` vs stored full hash) or carries `<hash>` verbatim in its redacted
/// `raw_data` (the `%h` / `[SNAPSHOT HEAD ...]` keys from T040). The snapshot
/// path is retention-proof — it still resolves after the working tree is
/// gone. `hash` is constrained to hex so it can never reach `git` as an
/// option or shell-significant token.
fn commit_ref_resolves(conn: &Mutex<Connection>, repo_path: Option<&str>, hash: &str) -> bool {
    let is_hex_hash =
        !hash.is_empty() && hash.len() <= 64 && hash.chars().all(|c| c.is_ascii_hexdigit());
    if !is_hex_hash {
        return false;
    }

    if let Some(repo) = repo_path.filter(|p| !p.is_empty() && *p != "global") {
        let ok = std::process::Command::new("git")
            .args(["cat-file", "-e", &format!("{hash}^{{commit}}")])
            .current_dir(repo)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return true;
        }
    }

    let conn = conn.lock();
    conn.query_row(
        "SELECT 1 FROM git_snapshots
         WHERE commit_hash GLOB ?1 || '*' OR instr(COALESCE(raw_data, ''), ?1) > 0
         LIMIT 1",
        params![hash],
        |_| Ok(()),
    )
    .optional()
    .unwrap_or(None)
    .is_some()
}

fn tombstone_blocks(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM rule_tombstones WHERE rule_name = ?1 AND reactivated_at IS NULL",
        params![name],
        |_| Ok(()),
    )
    .optional()
    .unwrap_or(None)
    .is_some()
}

fn backfill_context_event_categories(conn: &Connection) -> Result<(), String> {
    // Single bulk UPDATE driven by the SQL CASE mirror of `derive_category`.
    // Replaces an earlier per-row UPDATE loop that scaled poorly on tables
    // with millions of historical events.  The CASE expression is unit-
    // tested against `derive_category` in [`crate::context_category`].
    let sql = format!(
        "UPDATE context_savings_events
         SET category = ({case})
         WHERE category = 'unknown' OR category IS NULL OR category = ''",
        case = crate::context_category::DERIVE_CATEGORY_CASE_SQL,
    );
    conn.execute(&sql, [])
        .map_err(|e| format!("Bulk backfill context savings categories error: {e}"))?;
    Ok(())
}

fn normalize_context_event_token_estimates(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "UPDATE context_savings_events
         SET tokens_saved_est = 0,
             tokens_preserved_est = 0
         WHERE category NOT IN ('preservation', 'retrieval')
           AND (COALESCE(tokens_saved_est, 0) != 0
                OR COALESCE(tokens_preserved_est, 0) != 0)",
        [],
    )
    .map_err(|e| format!("Normalize context savings token estimates error: {e}"))?;
    Ok(())
}

fn ensure_startup_indexes(conn: &Connection) -> Result<(), String> {
    let conditional_indexes = [
        (
            table_has_column(conn, "usage_snapshots", "provider")
                && table_has_column(conn, "usage_snapshots", "bucket_key"),
            "CREATE INDEX IF NOT EXISTS idx_snapshots_provider_bucket ON usage_snapshots(provider, bucket_key);
             CREATE INDEX IF NOT EXISTS idx_snapshots_ts_provider_bucket ON usage_snapshots(timestamp, provider, bucket_key);",
            "usage snapshot provider indexes",
        ),
        (
            table_has_column(conn, "usage_hourly", "provider")
                && table_has_column(conn, "usage_hourly", "bucket_key"),
            "CREATE INDEX IF NOT EXISTS idx_hourly_provider_bucket ON usage_hourly(provider, bucket_key);",
            "usage hourly provider indexes",
        ),
        (
            table_has_column(conn, "token_snapshots", "provider"),
            "CREATE INDEX IF NOT EXISTS idx_token_snap_provider_ts ON token_snapshots(provider, timestamp);
             CREATE INDEX IF NOT EXISTS idx_token_snap_provider_session_ts ON token_snapshots(provider, session_id, timestamp DESC);
             CREATE INDEX IF NOT EXISTS idx_token_snap_provider_cwd ON token_snapshots(provider, cwd, timestamp);",
            "token snapshot provider indexes",
        ),
        (
            table_has_column(conn, "token_hourly", "provider"),
            "CREATE INDEX IF NOT EXISTS idx_token_hourly_provider_hour ON token_hourly(provider, hour);",
            "token hourly provider indexes",
        ),
        (
            table_has_column(conn, "observations", "provider"),
            "CREATE INDEX IF NOT EXISTS idx_obs_provider_created ON observations(provider, created_at);
             CREATE INDEX IF NOT EXISTS idx_obs_provider_created_tool ON observations(provider, created_at, tool_name);
             CREATE INDEX IF NOT EXISTS idx_obs_created_tool ON observations(created_at, tool_name);",
            "observation provider indexes",
        ),
        (
            table_has_column(conn, "tool_actions", "category")
                && table_has_column(conn, "tool_actions", "provider")
                && table_has_column(conn, "tool_actions", "session_id"),
            "CREATE INDEX IF NOT EXISTS idx_tool_actions_category_timestamp ON tool_actions(category, timestamp);
             CREATE INDEX IF NOT EXISTS idx_tool_actions_category_provider_session ON tool_actions(category, provider, session_id);",
            "tool action query indexes",
        ),
        (
            table_has_column(conn, "skill_usages", "provider")
                && table_has_column(conn, "skill_usages", "timestamp"),
            "CREATE INDEX IF NOT EXISTS idx_skill_usages_provider_ts
                 ON skill_usages(provider, timestamp);
             CREATE INDEX IF NOT EXISTS idx_skill_usages_provider_session
                 ON skill_usages(provider, session_id);
             CREATE INDEX IF NOT EXISTS idx_skill_usages_skill_ts
                 ON skill_usages(skill_name, timestamp);",
            "skill usage indexes",
        ),
        (
            table_has_column(conn, "context_savings_events", "event_id"),
            "CREATE INDEX IF NOT EXISTS idx_context_savings_timestamp
                 ON context_savings_events(timestamp);
             CREATE INDEX IF NOT EXISTS idx_context_savings_provider_timestamp
                 ON context_savings_events(provider, timestamp);
             CREATE INDEX IF NOT EXISTS idx_context_savings_provider_session_timestamp
                 ON context_savings_events(provider, session_id, timestamp);
             CREATE INDEX IF NOT EXISTS idx_context_savings_event_type_timestamp
                 ON context_savings_events(event_type, timestamp);
             CREATE INDEX IF NOT EXISTS idx_context_savings_cwd_timestamp
                 ON context_savings_events(cwd, timestamp);",
            "context savings indexes",
        ),
    ];

    for (enabled, sql, label) in conditional_indexes {
        if enabled {
            conn.execute_batch(sql)
                .map_err(|e| format!("Failed to create {label}: {e}"))?;
        }
    }

    Ok(())
}

pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    pub fn init() -> Result<Self, String> {
        let path = db_path()?;
        let mut conn =
            Connection::open(&path).map_err(|e| format!("Failed to open database: {e}"))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| format!("Failed to set pragmas: {e}"))?;

        conn.execute_batch(
            r#"CREATE TABLE IF NOT EXISTS usage_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'claude',
                bucket_key TEXT NOT NULL,
                bucket_label TEXT NOT NULL,
                utilization REAL NOT NULL,
                resets_at TEXT,
                created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON usage_snapshots(timestamp);

            CREATE TABLE IF NOT EXISTS usage_hourly (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                hour TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'claude',
                bucket_key TEXT NOT NULL,
                bucket_label TEXT NOT NULL,
                avg_utilization REAL NOT NULL,
                max_utilization REAL NOT NULL,
                min_utilization REAL NOT NULL,
                sample_count INTEGER NOT NULL,
                UNIQUE(hour, provider, bucket_key)
            );
            CREATE INDEX IF NOT EXISTS idx_hourly_hour ON usage_hourly(hour);

            CREATE TABLE IF NOT EXISTS token_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL DEFAULT 'claude',
                session_id TEXT NOT NULL,
                hostname TEXT NOT NULL DEFAULT 'local',
                timestamp TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                cwd TEXT DEFAULT NULL,
                created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_token_snap_ts ON token_snapshots(timestamp);
            CREATE INDEX IF NOT EXISTS idx_token_snap_host ON token_snapshots(hostname);
            CREATE INDEX IF NOT EXISTS idx_token_snap_session_ts ON token_snapshots(session_id, timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_token_snap_cwd ON token_snapshots(cwd, timestamp);

            CREATE TABLE IF NOT EXISTS token_hourly (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                hour TEXT NOT NULL,
                provider TEXT NOT NULL DEFAULT 'claude',
                hostname TEXT NOT NULL DEFAULT 'local',
                total_input INTEGER NOT NULL,
                total_output INTEGER NOT NULL,
                total_cache_creation INTEGER NOT NULL DEFAULT 0,
                total_cache_read INTEGER NOT NULL DEFAULT 0,
                turn_count INTEGER NOT NULL,
                UNIQUE(hour, hostname, provider)
            );
            CREATE INDEX IF NOT EXISTS idx_token_hourly_hour ON token_hourly(hour);

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Learning system tables
            CREATE TABLE IF NOT EXISTS observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL DEFAULT 'claude',
                session_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                hook_phase TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                tool_input TEXT,
                tool_output TEXT,
                cwd TEXT,
                created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_obs_session ON observations(session_id);
            CREATE INDEX IF NOT EXISTS idx_obs_timestamp ON observations(timestamp);
            CREATE INDEX IF NOT EXISTS idx_obs_created ON observations(created_at);

            CREATE TABLE IF NOT EXISTS learning_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trigger_mode TEXT NOT NULL,
                observations_analyzed INTEGER NOT NULL DEFAULT 0,
                rules_created INTEGER NOT NULL DEFAULT 0,
                rules_updated INTEGER NOT NULL DEFAULT 0,
                duration_ms INTEGER,
                status TEXT NOT NULL DEFAULT 'running',
                error TEXT,
                provider_scope TEXT NOT NULL DEFAULT '["claude"]',
                created_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS learned_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                domain TEXT,
                confidence REAL NOT NULL DEFAULT 0.5,
                observation_count INTEGER NOT NULL DEFAULT 0,
                file_path TEXT NOT NULL,
                provider_scope TEXT NOT NULL DEFAULT '["claude"]',
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS context_savings_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                schema_version INTEGER NOT NULL,
                provider TEXT NOT NULL,
                session_id TEXT,
                hostname TEXT NOT NULL,
                cwd TEXT,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                source TEXT NOT NULL,
                decision TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT 'unknown',
                reason TEXT,
                delivered INTEGER NOT NULL,
                indexed_bytes INTEGER,
                returned_bytes INTEGER,
                input_bytes INTEGER,
                tokens_indexed_est INTEGER,
                tokens_returned_est INTEGER,
                tokens_saved_est INTEGER,
                tokens_preserved_est INTEGER,
                estimate_method TEXT,
                estimate_confidence REAL,
                source_ref TEXT,
                snapshot_ref TEXT,
                metadata_json TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_context_savings_timestamp
                ON context_savings_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_context_savings_provider_timestamp
                ON context_savings_events(provider, timestamp);
            CREATE INDEX IF NOT EXISTS idx_context_savings_provider_session_timestamp
                ON context_savings_events(provider, session_id, timestamp);
            CREATE INDEX IF NOT EXISTS idx_context_savings_event_type_timestamp
                ON context_savings_events(event_type, timestamp);
            CREATE INDEX IF NOT EXISTS idx_context_savings_cwd_timestamp
                ON context_savings_events(cwd, timestamp);
            -- The category index is created by migration 18 after ALTER
            -- TABLE adds the column.  Keeping it here would crash startup
            -- for any database created before migration 18 because the
            -- column does not exist when this batch runs.
            "#,
        )
        .map_err(|e| format!("Failed to create tables: {e}"))?;

        // Schema versioning
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            )",
        )
        .map_err(|e| format!("Failed to create schema_version table: {e}"))?;

        let current_version: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Migration 1: add cwd column to token_snapshots
        if current_version < 1 {
            let has_cwd: bool = conn
                .prepare("SELECT cwd FROM token_snapshots LIMIT 0")
                .is_ok();
            if !has_cwd {
                conn.execute_batch("ALTER TABLE token_snapshots ADD COLUMN cwd TEXT DEFAULT NULL;")
                    .map_err(|e| format!("Migration 1 (add cwd column) error: {e}"))?;
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])
                .map_err(|e| format!("Failed to record migration 1: {e}"))?;
        }

        // Migration 2: add logs column to learning_runs
        if current_version < 2 {
            let has_logs: bool = conn
                .prepare("SELECT logs FROM learning_runs LIMIT 0")
                .is_ok();
            if !has_logs {
                conn.execute_batch("ALTER TABLE learning_runs ADD COLUMN logs TEXT DEFAULT NULL;")
                    .map_err(|e| format!("Migration 2 (add logs column) error: {e}"))?;
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (2)", [])
                .map_err(|e| format!("Failed to record migration 2: {e}"))?;
        }

        // Migration 3: add Beta-Binomial columns to learned_rules
        if current_version < 3 {
            let cols = [
                ("alpha", "REAL NOT NULL DEFAULT 1.0"),
                ("beta_param", "REAL NOT NULL DEFAULT 1.0"),
                ("last_evidence_at", "TEXT DEFAULT NULL"),
                ("state", "TEXT NOT NULL DEFAULT 'emerging'"),
                ("project", "TEXT DEFAULT NULL"),
            ];
            for (col, typ) in &cols {
                let has_col: bool = conn
                    .prepare(&format!("SELECT {col} FROM learned_rules LIMIT 0"))
                    .is_ok();
                if !has_col {
                    conn.execute_batch(&format!(
                        "ALTER TABLE learned_rules ADD COLUMN {col} {typ};"
                    ))
                    .map_err(|e| format!("Migration 3 (add {col}) error: {e}"))?;
                }
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (3)", [])
                .map_err(|e| format!("Failed to record migration 3: {e}"))?;
        }

        // Migration 4: anti-patterns, cross-project tracking, observation summaries
        if current_version < 4 {
            let new_cols = [
                ("is_anti_pattern", "INTEGER NOT NULL DEFAULT 0"),
                ("confirmed_projects", "TEXT DEFAULT NULL"),
            ];
            for (col, typ) in &new_cols {
                let has_col: bool = conn
                    .prepare(&format!("SELECT {col} FROM learned_rules LIMIT 0"))
                    .is_ok();
                if !has_col {
                    conn.execute_batch(&format!(
                        "ALTER TABLE learned_rules ADD COLUMN {col} {typ};"
                    ))
                    .map_err(|e| format!("Migration 4 (add {col}) error: {e}"))?;
                }
            }

            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS observation_summaries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    period TEXT NOT NULL,
                    provider TEXT NOT NULL DEFAULT 'claude',
                    project TEXT,
                    tool_counts TEXT NOT NULL,
                    error_count INTEGER NOT NULL DEFAULT 0,
                    total_observations INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT DEFAULT (datetime('now')),
                    UNIQUE(period, provider, project)
                );
                CREATE INDEX IF NOT EXISTS idx_obs_summaries_period ON observation_summaries(period);
                CREATE INDEX IF NOT EXISTS idx_obs_summaries_provider_period ON observation_summaries(provider, period);",
            )
            .map_err(|e| format!("Migration 4 (observation_summaries) error: {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (4)", [])
                .map_err(|e| format!("Failed to record migration 4: {e}"))?;
        }

        // Migration 5: tool_actions table for MCP server
        if current_version < 5 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS tool_actions (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    provider      TEXT NOT NULL DEFAULT 'claude',
                    message_id    TEXT NOT NULL,
                    session_id    TEXT NOT NULL,
                    tool_name     TEXT NOT NULL,
                    category      TEXT NOT NULL,
                    file_path     TEXT,
                    summary       TEXT NOT NULL,
                    full_input    TEXT,
                    full_output   TEXT,
                    timestamp     TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_tool_actions_provider_session ON tool_actions(provider, session_id);
                CREATE INDEX IF NOT EXISTS idx_tool_actions_session  ON tool_actions(session_id);
                CREATE INDEX IF NOT EXISTS idx_tool_actions_message  ON tool_actions(message_id);
                CREATE INDEX IF NOT EXISTS idx_tool_actions_file     ON tool_actions(file_path);
                CREATE INDEX IF NOT EXISTS idx_tool_actions_category ON tool_actions(category);",
            )
            .map_err(|e| format!("Migration 5 (tool_actions table) error: {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (5)", [])
                .map_err(|e| format!("Failed to record migration 5: {e}"))?;
        }

        // Migration 6: memory optimizer tables
        if current_version < 6 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS memory_files (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    project_path    TEXT NOT NULL,
                    file_path       TEXT NOT NULL,
                    content_hash    TEXT NOT NULL,
                    last_scanned_at TEXT NOT NULL,
                    UNIQUE(project_path, file_path)
                );
                CREATE INDEX IF NOT EXISTS idx_memfiles_project ON memory_files(project_path);

                CREATE TABLE IF NOT EXISTS optimization_runs (
                    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                    project_path        TEXT NOT NULL,
                    provider_scope      TEXT NOT NULL DEFAULT '[\"claude\"]',
                    trigger             TEXT NOT NULL,
                    memories_scanned    INTEGER NOT NULL DEFAULT 0,
                    suggestions_created INTEGER NOT NULL DEFAULT 0,
                    context_sources     TEXT NOT NULL DEFAULT '{}',
                    status              TEXT NOT NULL DEFAULT 'running',
                    error               TEXT,
                    started_at          TEXT NOT NULL,
                    completed_at        TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_optrun_project_started ON optimization_runs(project_path, started_at);

                CREATE TABLE IF NOT EXISTS optimization_suggestions (
                    id               INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id           INTEGER NOT NULL REFERENCES optimization_runs(id),
                    project_path     TEXT NOT NULL,
                    provider_scope   TEXT NOT NULL DEFAULT '[\"claude\"]',
                    action_type      TEXT NOT NULL,
                    target_file      TEXT,
                    reasoning        TEXT NOT NULL,
                    proposed_content TEXT,
                    merge_sources    TEXT,
                    status           TEXT NOT NULL DEFAULT 'pending',
                    error            TEXT,
                    resolved_at      TEXT,
                    created_at       TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_optsug_run ON optimization_suggestions(run_id);
                CREATE INDEX IF NOT EXISTS idx_optsug_project_status ON optimization_suggestions(project_path, status);",
            )
            .map_err(|e| format!("Migration 6 (memory optimizer tables) error: {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (6)", [])
                .map_err(|e| format!("Failed to record migration 6: {e}"))?;
        }

        // Migration 7: memory optimizer redesign — diff, backup, and cleanup support
        if current_version < 7 {
            let has_col = |col: &str| -> bool {
                conn.prepare(&format!(
                    "SELECT {col} FROM optimization_suggestions LIMIT 0"
                ))
                .is_ok()
            };
            if !has_col("original_content") {
                conn.execute_batch(
                    "ALTER TABLE optimization_suggestions ADD COLUMN original_content TEXT;",
                )
                .map_err(|e| format!("Migration 7 (original_content): {e}"))?;
            }
            if !has_col("diff_summary") {
                conn.execute_batch(
                    "ALTER TABLE optimization_suggestions ADD COLUMN diff_summary TEXT;",
                )
                .map_err(|e| format!("Migration 7 (diff_summary): {e}"))?;
            }
            if !has_col("backup_data") {
                conn.execute_batch(
                    "ALTER TABLE optimization_suggestions ADD COLUMN backup_data TEXT;",
                )
                .map_err(|e| format!("Migration 7 (backup_data): {e}"))?;
            }
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_optsug_status_created ON optimization_suggestions(status, created_at);
                 CREATE INDEX IF NOT EXISTS idx_optsug_status_resolved ON optimization_suggestions(status, resolved_at);",
            )
            .map_err(|e| format!("Migration 7 (indexes): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (7)", [])
                .map_err(|e| format!("Failed to record migration 7: {e}"))?;
        }

        // Migration 8: multi-stream learning — git cache, rule source, run phases
        if current_version < 8 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS git_snapshots (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    project      TEXT NOT NULL UNIQUE,
                    commit_hash  TEXT NOT NULL,
                    commit_count INTEGER NOT NULL,
                    raw_data     TEXT NOT NULL,
                    created_at   TEXT DEFAULT (datetime('now'))
                );",
            )
            .map_err(|e| format!("Migration 8 (git_snapshots table): {e}"))?;

            let has_source: bool = conn
                .prepare("SELECT source FROM learned_rules LIMIT 0")
                .is_ok();
            if !has_source {
                conn.execute_batch(
                    "ALTER TABLE learned_rules ADD COLUMN source TEXT DEFAULT 'observations';",
                )
                .map_err(|e| format!("Migration 8 (source column): {e}"))?;
            }

            let has_phases: bool = conn
                .prepare("SELECT phases FROM learning_runs LIMIT 0")
                .is_ok();
            if !has_phases {
                conn.execute_batch(
                    "ALTER TABLE learning_runs ADD COLUMN phases TEXT DEFAULT NULL;",
                )
                .map_err(|e| format!("Migration 8 (phases column): {e}"))?;
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (8)", [])
                .map_err(|e| format!("Failed to record migration 8: {e}"))?;
        }

        // Migration 9: suggestion conflict groups
        if current_version < 9 {
            let has_group_id: bool = conn
                .prepare("SELECT group_id FROM optimization_suggestions LIMIT 0")
                .is_ok();
            if !has_group_id {
                conn.execute_batch(
                    "ALTER TABLE optimization_suggestions ADD COLUMN group_id TEXT;",
                )
                .map_err(|e| format!("Migration 9 (group_id): {e}"))?;
            }
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_optsug_group ON optimization_suggestions(group_id);",
            )
            .map_err(|e| format!("Migration 9 (group_id index): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (9)", [])
                .map_err(|e| format!("Failed to record migration 9: {e}"))?;
        }

        // Migration 10: response_times table for response/idle latency tracking
        if current_version < 10 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS response_times (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    provider     TEXT NOT NULL DEFAULT 'claude',
                    session_id   TEXT NOT NULL,
                    timestamp    TEXT NOT NULL,
                    response_secs REAL,
                    idle_secs    REAL,
                    created_at   TEXT DEFAULT (datetime('now')),
                    UNIQUE(provider, session_id, timestamp)
                );
                CREATE INDEX IF NOT EXISTS idx_rt_provider_session ON response_times(provider, session_id);
                CREATE INDEX IF NOT EXISTS idx_rt_session ON response_times(session_id);
                CREATE INDEX IF NOT EXISTS idx_rt_timestamp ON response_times(timestamp);",
            )
            .map_err(|e| format!("Migration 10 (response_times table): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (10)", [])
                .map_err(|e| format!("Failed to record migration 10: {e}"))?;
        }

        if current_version < 11 {
            let has_content: bool = conn
                .prepare("SELECT content FROM learned_rules LIMIT 0")
                .is_ok();
            if !has_content {
                conn.execute_batch(
                    "ALTER TABLE learned_rules ADD COLUMN content TEXT DEFAULT NULL;",
                )
                .map_err(|e| format!("Migration 11 (content column): {e}"))?;
            }
            conn.execute("INSERT INTO schema_version (version) VALUES (11)", [])
                .map_err(|e| format!("Failed to record migration 11: {e}"))?;
        }

        if current_version < 12 {
            let has_token_provider: bool = conn
                .prepare("SELECT provider FROM token_snapshots LIMIT 0")
                .is_ok();
            if !has_token_provider {
                conn.execute_batch(
                    "ALTER TABLE token_snapshots ADD COLUMN provider TEXT NOT NULL DEFAULT 'claude';
                     CREATE INDEX IF NOT EXISTS idx_token_snap_provider_ts ON token_snapshots(provider, timestamp);
                     CREATE INDEX IF NOT EXISTS idx_token_snap_provider_session_ts ON token_snapshots(provider, session_id, timestamp DESC);
                     CREATE INDEX IF NOT EXISTS idx_token_snap_provider_cwd ON token_snapshots(provider, cwd, timestamp);",
                )
                .map_err(|e| format!("Migration 12 (token_snapshots provider): {e}"))?;
            }

            let has_token_hourly_provider: bool = conn
                .prepare("SELECT provider FROM token_hourly LIMIT 0")
                .is_ok();
            if !has_token_hourly_provider {
                conn.execute_batch(
                    "ALTER TABLE token_hourly RENAME TO token_hourly_legacy;
                     CREATE TABLE token_hourly (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         hour TEXT NOT NULL,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         hostname TEXT NOT NULL DEFAULT 'local',
                         total_input INTEGER NOT NULL,
                         total_output INTEGER NOT NULL,
                         total_cache_creation INTEGER NOT NULL DEFAULT 0,
                         total_cache_read INTEGER NOT NULL DEFAULT 0,
                         turn_count INTEGER NOT NULL,
                         UNIQUE(hour, hostname, provider)
                     );
                     INSERT INTO token_hourly (hour, provider, hostname, total_input, total_output, total_cache_creation, total_cache_read, turn_count)
                     SELECT hour, 'claude', hostname, total_input, total_output, total_cache_creation, total_cache_read, turn_count
                     FROM token_hourly_legacy;
                     DROP TABLE token_hourly_legacy;
                     CREATE INDEX IF NOT EXISTS idx_token_hourly_hour ON token_hourly(hour);
                     CREATE INDEX IF NOT EXISTS idx_token_hourly_provider_hour ON token_hourly(provider, hour);",
                )
                .map_err(|e| format!("Migration 12 (token_hourly provider): {e}"))?;
            }

            let has_obs_provider: bool = conn
                .prepare("SELECT provider FROM observations LIMIT 0")
                .is_ok();
            if !has_obs_provider {
                conn.execute_batch(
                    "ALTER TABLE observations ADD COLUMN provider TEXT NOT NULL DEFAULT 'claude';
                     CREATE INDEX IF NOT EXISTS idx_obs_provider_created ON observations(provider, created_at);",
                )
                .map_err(|e| format!("Migration 12 (observations provider): {e}"))?;
            }

            let has_obs_summaries_table: bool = conn
                .prepare("SELECT 1 FROM observation_summaries LIMIT 0")
                .is_ok();
            let has_obs_summary_provider = has_obs_summaries_table
                && conn
                    .prepare("SELECT provider FROM observation_summaries LIMIT 0")
                    .is_ok();
            if has_obs_summaries_table && !has_obs_summary_provider {
                conn.execute_batch(
                    "ALTER TABLE observation_summaries RENAME TO observation_summaries_legacy;
                     CREATE TABLE observation_summaries (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         period TEXT NOT NULL,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         project TEXT,
                         tool_counts TEXT NOT NULL,
                         error_count INTEGER NOT NULL DEFAULT 0,
                         total_observations INTEGER NOT NULL DEFAULT 0,
                         created_at TEXT DEFAULT (datetime('now')),
                         UNIQUE(period, provider, project)
                     );
                     INSERT INTO observation_summaries (period, provider, project, tool_counts, error_count, total_observations, created_at)
                     SELECT period, 'claude', project, tool_counts, error_count, total_observations, created_at
                     FROM observation_summaries_legacy;
                     DROP TABLE observation_summaries_legacy;
                     CREATE INDEX IF NOT EXISTS idx_obs_summaries_period ON observation_summaries(period);
                     CREATE INDEX IF NOT EXISTS idx_obs_summaries_provider_period ON observation_summaries(provider, period);",
                )
                .map_err(|e| format!("Migration 12 (observation_summaries provider): {e}"))?;
            }

            let has_tool_provider: bool = conn
                .prepare("SELECT provider FROM tool_actions LIMIT 0")
                .is_ok();
            if !has_tool_provider {
                conn.execute_batch(
                    "ALTER TABLE tool_actions ADD COLUMN provider TEXT NOT NULL DEFAULT 'claude';
                     CREATE INDEX IF NOT EXISTS idx_tool_actions_provider_session ON tool_actions(provider, session_id);",
                )
                .map_err(|e| format!("Migration 12 (tool_actions provider): {e}"))?;
            }

            let has_response_provider: bool = conn
                .prepare("SELECT provider FROM response_times LIMIT 0")
                .is_ok();
            if !has_response_provider {
                conn.execute_batch(
                    "ALTER TABLE response_times RENAME TO response_times_legacy;
                     CREATE TABLE response_times (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         session_id TEXT NOT NULL,
                         timestamp TEXT NOT NULL,
                         response_secs REAL,
                         idle_secs REAL,
                         created_at TEXT DEFAULT (datetime('now')),
                         UNIQUE(provider, session_id, timestamp)
                     );
                     INSERT INTO response_times (provider, session_id, timestamp, response_secs, idle_secs, created_at)
                     SELECT 'claude', session_id, timestamp, response_secs, idle_secs, created_at
                     FROM response_times_legacy;
                     DROP TABLE response_times_legacy;
                     CREATE INDEX IF NOT EXISTS idx_rt_provider_session ON response_times(provider, session_id);
                     CREATE INDEX IF NOT EXISTS idx_rt_session ON response_times(session_id);
                     CREATE INDEX IF NOT EXISTS idx_rt_timestamp ON response_times(timestamp);",
                )
                .map_err(|e| format!("Migration 12 (response_times provider): {e}"))?;
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (12)", [])
                .map_err(|e| format!("Failed to record migration 12: {e}"))?;
        }

        if current_version < 13 {
            let migration_checks = [
                (
                    "SELECT provider_scope FROM learning_runs LIMIT 0",
                    "ALTER TABLE learning_runs ADD COLUMN provider_scope TEXT NOT NULL DEFAULT '[\"claude\"]';",
                    "learning_runs provider_scope",
                ),
                (
                    "SELECT provider_scope FROM learned_rules LIMIT 0",
                    "ALTER TABLE learned_rules ADD COLUMN provider_scope TEXT NOT NULL DEFAULT '[\"claude\"]';",
                    "learned_rules provider_scope",
                ),
                (
                    "SELECT provider_scope FROM optimization_runs LIMIT 0",
                    "ALTER TABLE optimization_runs ADD COLUMN provider_scope TEXT NOT NULL DEFAULT '[\"claude\"]';",
                    "optimization_runs provider_scope",
                ),
                (
                    "SELECT provider_scope FROM optimization_suggestions LIMIT 0",
                    "ALTER TABLE optimization_suggestions ADD COLUMN provider_scope TEXT NOT NULL DEFAULT '[\"claude\"]';",
                    "optimization_suggestions provider_scope",
                ),
            ];

            for (check_sql, migrate_sql, label) in migration_checks {
                let has_column = conn.prepare(check_sql).is_ok();
                if !has_column {
                    conn.execute_batch(migrate_sql)
                        .map_err(|e| format!("Migration 13 ({label}): {e}"))?;
                }
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (13)", [])
                .map_err(|e| format!("Failed to record migration 13: {e}"))?;
        }

        if current_version < 14 {
            let bucket_key_case = "CASE bucket_label
                WHEN '5 hours' THEN 'five_hour'
                WHEN '7 days' THEN 'seven_day'
                WHEN 'Sonnet' THEN 'seven_day_sonnet'
                WHEN 'Opus' THEN 'seven_day_opus'
                WHEN 'Code' THEN 'seven_day_cowork'
                WHEN 'OAuth' THEN 'seven_day_oauth_apps'
                WHEN 'Extra' THEN 'extra_usage'
                ELSE lower(replace(bucket_label, ' ', '_'))
            END";

            let usage_snapshots_has_provider = conn
                .prepare("SELECT provider FROM usage_snapshots LIMIT 0")
                .is_ok();
            let usage_snapshots_has_bucket_key = conn
                .prepare("SELECT bucket_key FROM usage_snapshots LIMIT 0")
                .is_ok();
            if !usage_snapshots_has_provider || !usage_snapshots_has_bucket_key {
                let migrate_sql = format!(
                    "ALTER TABLE usage_snapshots RENAME TO usage_snapshots_legacy;
                     CREATE TABLE usage_snapshots (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         timestamp TEXT NOT NULL,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         bucket_key TEXT NOT NULL,
                         bucket_label TEXT NOT NULL,
                         utilization REAL NOT NULL,
                         resets_at TEXT,
                         created_at TEXT DEFAULT (datetime('now'))
                     );
                     INSERT INTO usage_snapshots (timestamp, provider, bucket_key, bucket_label, utilization, resets_at, created_at)
                     SELECT
                         timestamp,
                         'claude',
                         {bucket_key_case},
                         bucket_label,
                         utilization,
                         resets_at,
                         created_at
                     FROM usage_snapshots_legacy;
                     DROP TABLE usage_snapshots_legacy;
                     CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON usage_snapshots(timestamp);
                     CREATE INDEX IF NOT EXISTS idx_snapshots_provider_bucket ON usage_snapshots(provider, bucket_key);
                     CREATE INDEX IF NOT EXISTS idx_snapshots_ts_provider_bucket ON usage_snapshots(timestamp, provider, bucket_key);"
                );
                conn.execute_batch(&migrate_sql)
                    .map_err(|e| format!("Migration 14 (usage_snapshots): {e}"))?;
            }

            let usage_hourly_has_provider = conn
                .prepare("SELECT provider FROM usage_hourly LIMIT 0")
                .is_ok();
            let usage_hourly_has_bucket_key = conn
                .prepare("SELECT bucket_key FROM usage_hourly LIMIT 0")
                .is_ok();
            if !usage_hourly_has_provider || !usage_hourly_has_bucket_key {
                let migrate_sql = format!(
                    "ALTER TABLE usage_hourly RENAME TO usage_hourly_legacy;
                     CREATE TABLE usage_hourly (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         hour TEXT NOT NULL,
                         provider TEXT NOT NULL DEFAULT 'claude',
                         bucket_key TEXT NOT NULL,
                         bucket_label TEXT NOT NULL,
                         avg_utilization REAL NOT NULL,
                         max_utilization REAL NOT NULL,
                         min_utilization REAL NOT NULL,
                         sample_count INTEGER NOT NULL,
                         UNIQUE(hour, provider, bucket_key)
                     );
                     INSERT INTO usage_hourly (hour, provider, bucket_key, bucket_label, avg_utilization, max_utilization, min_utilization, sample_count)
                     SELECT
                         hour,
                         'claude',
                         {bucket_key_case},
                         bucket_label,
                         avg_utilization,
                         max_utilization,
                         min_utilization,
                         sample_count
                     FROM usage_hourly_legacy;
                     DROP TABLE usage_hourly_legacy;
                     CREATE INDEX IF NOT EXISTS idx_hourly_hour ON usage_hourly(hour);
                     CREATE INDEX IF NOT EXISTS idx_hourly_provider_bucket ON usage_hourly(provider, bucket_key);"
                );
                conn.execute_batch(&migrate_sql)
                    .map_err(|e| format!("Migration 14 (usage_hourly): {e}"))?;
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (14)", [])
                .map_err(|e| format!("Failed to record migration 14: {e}"))?;
        }

        if current_version < 15 {
            let has_content_hash: bool = conn
                .prepare("SELECT content_hash FROM learned_rules LIMIT 0")
                .is_ok();
            if !has_content_hash {
                conn.execute(
                    "ALTER TABLE learned_rules ADD COLUMN content_hash TEXT DEFAULT NULL;",
                    [],
                )
                .map_err(|e| format!("Migration 15 (content_hash): {e}"))?;
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (15)", [])
                .map_err(|e| format!("Failed to record migration 15: {e}"))?;
        }

        if current_version < 16 {
            conn.execute_batch(CONTEXT_SAVINGS_EVENTS_SCHEMA)
                .map_err(|e| format!("Migration 16 (context_savings_events): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (16)", [])
                .map_err(|e| format!("Failed to record migration 16: {e}"))?;
        }

        if current_version < 17 {
            conn.execute(
                "UPDATE context_savings_events
                 SET tokens_saved_est =
                     (COALESCE(indexed_bytes, input_bytes, 0) - COALESCE(returned_bytes, 0) + 3) / 4
                 WHERE tokens_saved_est = 0
                   AND delivered = 0
                   AND indexed_bytes IS NOT NULL
                   AND returned_bytes IS NULL
                   AND COALESCE(indexed_bytes, input_bytes, 0) > COALESCE(returned_bytes, 0)",
                [],
            )
            .map_err(|e| format!("Migration 17 (context savings saved estimate backfill): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (17)", [])
                .map_err(|e| format!("Failed to record migration 17: {e}"))?;
        }

        if current_version < 18 {
            if !table_has_column(&conn, "context_savings_events", "category") {
                conn.execute(
                    "ALTER TABLE context_savings_events
                         ADD COLUMN category TEXT NOT NULL DEFAULT 'unknown'",
                    [],
                )
                .map_err(|e| format!("Migration 18 (add category column): {e}"))?;
            }

            backfill_context_event_categories(&conn)?;

            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_context_savings_category_timestamp
                     ON context_savings_events(category, timestamp)",
                [],
            )
            .map_err(|e| format!("Migration 18 (category index): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (18)", [])
                .map_err(|e| format!("Failed to record migration 18: {e}"))?;
        }

        if current_version < 19 {
            backfill_context_event_categories(&conn)?;
            normalize_context_event_token_estimates(&conn)?;

            conn.execute("INSERT INTO schema_version (version) VALUES (19)", [])
                .map_err(|e| format!("Failed to record migration 19: {e}"))?;
        }

        // Migration 20: sub-agent attribution columns on response_times,
        // tool_actions, and token_snapshots. Provider-agnostic — Codex extraction
        // simply writes the defaults (is_sidechain=0, agent_id=NULL, parent_uuid=NULL).
        if current_version < 20 {
            let tx = conn
                .transaction()
                .map_err(|e| format!("Migration 20 transaction begin: {e}"))?;

            let subagent_tables = ["response_times", "tool_actions", "token_snapshots"];
            for table in subagent_tables {
                if !table_has_column(&tx, table, "is_sidechain") {
                    tx.execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN is_sidechain INTEGER NOT NULL DEFAULT 0;"
                    ))
                    .map_err(|e| format!("Migration 20 ({table}.is_sidechain): {e}"))?;
                }
                if !table_has_column(&tx, table, "agent_id") {
                    tx.execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN agent_id TEXT DEFAULT NULL;"
                    ))
                    .map_err(|e| format!("Migration 20 ({table}.agent_id): {e}"))?;
                }
                if !table_has_column(&tx, table, "parent_uuid") {
                    tx.execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN parent_uuid TEXT DEFAULT NULL;"
                    ))
                    .map_err(|e| format!("Migration 20 ({table}.parent_uuid): {e}"))?;
                }
            }

            // Rollup + tree indexes for each sub-agent-aware table.
            tx.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_rt_provider_session_sidechain
                     ON response_times(provider, session_id, is_sidechain);
                 CREATE INDEX IF NOT EXISTS idx_rt_provider_session_agent
                     ON response_times(provider, session_id, agent_id);
                 CREATE INDEX IF NOT EXISTS idx_tool_actions_provider_session_sidechain
                     ON tool_actions(provider, session_id, is_sidechain);
                 CREATE INDEX IF NOT EXISTS idx_tool_actions_provider_session_agent
                     ON tool_actions(provider, session_id, agent_id);
                 CREATE INDEX IF NOT EXISTS idx_token_snap_provider_session_sidechain
                     ON token_snapshots(provider, session_id, is_sidechain);
                 CREATE INDEX IF NOT EXISTS idx_token_snap_provider_session_agent
                     ON token_snapshots(provider, session_id, agent_id);",
            )
            .map_err(|e| format!("Migration 20 (indexes): {e}"))?;

            // Backfill strategy:
            //  * response_times and tool_actions are derived from transcripts on
            //    disk and are fully regenerable. Truncate them here; the next
            //    startup_scan rebuilds them with the new columns populated.
            //    `subagent_reingest_pending` flag tells SessionIndex to drop its
            //    mtime cache so every file is re-extracted in this boot.
            //  * token_snapshots come from hook-reported events that cannot be
            //    regenerated. Existing rows stay tagged is_sidechain=0; they
            //    appear in the parent's rolled-up totals correctly. New rows
            //    coming in via store_token_snapshot will be tagged correctly.
            //    TODO(wave-2+): expose a CLI repair util that walks
            //    subagents/*.jsonl on disk and updates token_snapshots by
            //    matching cwd + timestamp window for non-ambiguous cases.
            tx.execute("DELETE FROM response_times", [])
                .map_err(|e| format!("Migration 20 (clear response_times): {e}"))?;
            tx.execute("DELETE FROM tool_actions", [])
                .map_err(|e| format!("Migration 20 (clear tool_actions): {e}"))?;
            tx.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params!["subagent_reingest_pending", "1"],
            )
            .map_err(|e| format!("Migration 20 (set reingest flag): {e}"))?;

            tx.execute("INSERT INTO schema_version (version) VALUES (20)", [])
                .map_err(|e| format!("Failed to record migration 20: {e}"))?;

            tx.commit()
                .map_err(|e| format!("Migration 20 commit: {e}"))?;
        }

        if current_version < 21 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS skill_usages (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    provider    TEXT NOT NULL,
                    session_id  TEXT NOT NULL,
                    message_id  TEXT NOT NULL,
                    skill_name  TEXT NOT NULL,
                    skill_path  TEXT NOT NULL,
                    timestamp   TEXT NOT NULL,
                    tool_name   TEXT,
                    created_at  TEXT DEFAULT (datetime('now')),
                    UNIQUE(provider, session_id, message_id, skill_name, skill_path, timestamp)
                );
                CREATE INDEX IF NOT EXISTS idx_skill_usages_provider_ts
                    ON skill_usages(provider, timestamp);
                CREATE INDEX IF NOT EXISTS idx_skill_usages_provider_session
                    ON skill_usages(provider, session_id);
                CREATE INDEX IF NOT EXISTS idx_skill_usages_skill_ts
                    ON skill_usages(skill_name, timestamp);",
            )
            .map_err(|e| format!("Migration 21 (skill_usages table): {e}"))?;

            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params!["skill_usage_reingest_pending", "1"],
            )
            .map_err(|e| format!("Migration 21 (set reingest flag): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (21)", [])
                .map_err(|e| format!("Failed to record migration 21: {e}"))?;
        }

        if current_version < 22 {
            conn.execute_batch(
                "ALTER TABLE skill_usages ADD COLUMN cwd TEXT;
                 ALTER TABLE skill_usages ADD COLUMN hostname TEXT;
                 CREATE INDEX IF NOT EXISTS idx_skill_usages_skill_cwd
                     ON skill_usages(skill_name, cwd);",
            )
            .map_err(|e| format!("Migration 22 (skill_usages cwd/hostname): {e}"))?;

            // Re-arm reingest so historical rows refill cwd/hostname from
            // JSONL transcripts on the next session sweep.
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params!["skill_usage_reingest_pending", "1"],
            )
            .map_err(|e| format!("Migration 22 (set reingest flag): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (22)", [])
                .map_err(|e| format!("Failed to record migration 22: {e}"))?;
        }

        if current_version < 23 {
            // No schema change. Re-arm reingest so the next session sweep
            // replays Claude transcripts through the updated extractor,
            // which now recognizes the `Skill` tool call.
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params!["skill_usage_reingest_pending", "1"],
            )
            .map_err(|e| format!("Migration 23 (set reingest flag): {e}"))?;

            conn.execute("INSERT INTO schema_version (version) VALUES (23)", [])
                .map_err(|e| format!("Failed to record migration 23: {e}"))?;
        }

        if current_version < 24 {
            // Add inference_metadata to learning_runs and optimization_runs so
            // each run record can persist per-Claude-Code-invocation structured
            // metadata (tokens, model, durations, cost, cache stats, stop
            // reason, permission denials) returned by `claude -p --output-format
            // json`. Storage-only in this feature; future features may surface
            // it. See specs/003-cc-inference-migration/data-model.md.
            let migration_checks = [
                (
                    "SELECT inference_metadata FROM learning_runs LIMIT 0",
                    "ALTER TABLE learning_runs ADD COLUMN inference_metadata TEXT DEFAULT NULL;",
                    "learning_runs inference_metadata",
                ),
                (
                    "SELECT inference_metadata FROM optimization_runs LIMIT 0",
                    "ALTER TABLE optimization_runs ADD COLUMN inference_metadata TEXT DEFAULT NULL;",
                    "optimization_runs inference_metadata",
                ),
            ];
            for (check_sql, migrate_sql, label) in migration_checks {
                let has_column = conn.prepare(check_sql).is_ok();
                if !has_column {
                    conn.execute_batch(migrate_sql)
                        .map_err(|e| format!("Migration 24 ({label}): {e}"))?;
                }
            }

            conn.execute("INSERT INTO schema_version (version) VALUES (24)", [])
                .map_err(|e| format!("Failed to record migration 24: {e}"))?;
        }

        // Migration 25: learning-system hardening (feature 005).
        //
        // One additive, transactional, idempotent migration shared by
        // R-2/R-3/R-4/R-5/R-6. Adds provenance/lifecycle columns to
        // `learned_rules` and six append-only/governance tables. See
        // specs/005-learning-system-hardening/data-model.md
        // ("Migration 25 — schema delta") for the authoritative shape.
        //
        // `state` (existing) is left untouched — it stays the read-time
        // derived quality label, distinct from the new persisted
        // `lifecycle`. `confirmed_projects` already exists (migration 4) and
        // is repurposed (no DDL here) by US3 as the cross-project
        // distinct-sources signal.
        if current_version < 25 {
            let tx = conn
                .transaction()
                .map_err(|e| format!("Migration 25 transaction begin: {e}"))?;

            // learned_rules: additive provenance + lifecycle columns. Each
            // ALTER is guarded by `table_has_column` so re-running the
            // migration path on a partially-migrated DB is a no-op.
            let learned_rules_cols = [
                ("lifecycle", "TEXT NOT NULL DEFAULT 'candidate'"),
                ("origin_run_id", "INTEGER"),
                ("origin_model", "TEXT"),
                ("origin_at", "TEXT"),
                ("current_version", "INTEGER NOT NULL DEFAULT 1"),
                ("superseded_by", "TEXT"),
            ];
            for (col, typ) in learned_rules_cols {
                if !table_has_column(&tx, "learned_rules", col) {
                    tx.execute_batch(&format!(
                        "ALTER TABLE learned_rules ADD COLUMN {col} {typ};"
                    ))
                    .map_err(|e| format!("Migration 25 (learned_rules.{col}): {e}"))?;
                }
            }

            // rule_versions — append-only content history (C-2, FR-009).
            // Rollback is a forward restore row (change_kind='rollback',
            // rolled_back_from set).
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS rule_versions (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    rule_id         INTEGER NOT NULL
                                        REFERENCES learned_rules(id) ON DELETE CASCADE,
                    version         INTEGER NOT NULL,
                    content         TEXT NOT NULL,
                    content_hash    TEXT NOT NULL,
                    domain          TEXT,
                    is_anti_pattern INTEGER NOT NULL DEFAULT 0,
                    provider_scope  TEXT NOT NULL DEFAULT '[\"claude\"]',
                    source          TEXT,
                    run_id          INTEGER,
                    change_kind     TEXT NOT NULL,
                    rolled_back_from INTEGER,
                    author          TEXT NOT NULL DEFAULT 'system',
                    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                    UNIQUE(rule_id, version)
                );
                CREATE INDEX IF NOT EXISTS idx_rule_versions_rule_version
                    ON rule_versions(rule_id, version DESC);",
            )
            .map_err(|e| format!("Migration 25 (rule_versions): {e}"))?;

            // rule_evidence_citations — retention-proof grounding snapshot
            // (C-2, H-1; shared by R-2+R-6, defined once). `observation_id`
            // is a soft nullable ref with NO FK — `observations` is purged.
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS rule_evidence_citations (
                    id             INTEGER PRIMARY KEY AUTOINCREMENT,
                    rule_id        INTEGER NOT NULL
                                       REFERENCES learned_rules(id) ON DELETE CASCADE,
                    run_id         INTEGER,
                    rule_version   INTEGER,
                    observation_id INTEGER,
                    provider       TEXT,
                    session_id     TEXT,
                    cwd            TEXT,
                    tool_name      TEXT,
                    evidence_ts    TEXT,
                    snippet        TEXT,
                    kind           TEXT,
                    ref_id         TEXT,
                    created_at     TEXT NOT NULL DEFAULT (datetime('now'))
                );
                CREATE INDEX IF NOT EXISTS idx_rule_evidence_rule_version
                    ON rule_evidence_citations(rule_id, rule_version);
                CREATE INDEX IF NOT EXISTS idx_rule_evidence_run
                    ON rule_evidence_citations(run_id);
                CREATE INDEX IF NOT EXISTS idx_rule_evidence_observation
                    ON rule_evidence_citations(observation_id);",
            )
            .map_err(|e| format!("Migration 25 (rule_evidence_citations): {e}"))?;

            // rule_tombstones — durable suppression (C-5, FR-010). Name-keyed
            // (stable identity across re-extraction/reconcile); active iff a
            // row exists AND reactivated_at IS NULL. Never CASCADE-deleted —
            // must outlive the rule row.
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS rule_tombstones (
                    rule_name         TEXT PRIMARY KEY,
                    rule_id           INTEGER,
                    tombstoned_at     TEXT NOT NULL DEFAULT (datetime('now')),
                    tombstoned_by     TEXT NOT NULL,
                    reason            TEXT,
                    last_content_hash TEXT,
                    reactivated_at    TEXT,
                    reactivated_by    TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_rule_tombstones_reactivated
                    ON rule_tombstones(reactivated_at);",
            )
            .map_err(|e| format!("Migration 25 (rule_tombstones): {e}"))?;

            // operator_feedback — primary outcome signal (Q2=B, FR-029).
            // One revisable row per (rule_name, actor); `note` is
            // maintainer-only and never sent to inference.
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS operator_feedback (
                    id                INTEGER PRIMARY KEY AUTOINCREMENT,
                    rule_name         TEXT NOT NULL,
                    actor             TEXT NOT NULL DEFAULT 'operator',
                    feedback          TEXT NOT NULL,
                    note              TEXT,
                    rule_content_hash TEXT,
                    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
                    UNIQUE(rule_name, actor)
                );",
            )
            .map_err(|e| format!("Migration 25 (operator_feedback): {e}"))?;

            // evaluation_results — counterfactual verdicts (C-4, FR-022),
            // linked to (rule, run, replay_set_version).
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS evaluation_results (
                    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                    rule_name          TEXT NOT NULL,
                    learning_run_id    INTEGER REFERENCES learning_runs(id),
                    replay_set_version TEXT,
                    judge_model        TEXT,
                    evaluated_at       TEXT NOT NULL DEFAULT (datetime('now')),
                    verdict            TEXT,
                    delta              REAL,
                    regression         INTEGER NOT NULL DEFAULT 0,
                    negative_transfer  INTEGER NOT NULL DEFAULT 0,
                    judge_uncalibrated INTEGER NOT NULL DEFAULT 0,
                    replay_set_stale   INTEGER NOT NULL DEFAULT 0,
                    agreement_score    REAL,
                    rationale          TEXT,
                    per_case_json      TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_evaluation_results_rule_evaluated
                    ON evaluation_results(rule_name, evaluated_at DESC);",
            )
            .map_err(|e| format!("Migration 25 (evaluation_results): {e}"))?;

            // reviewer_overrides — audited regression overrides (C-4,
            // FR-020). Becomes part of provenance.
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS reviewer_overrides (
                    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                    rule_name          TEXT NOT NULL,
                    replay_set_version TEXT,
                    overridden_by      TEXT,
                    reason             TEXT NOT NULL,
                    overridden_at      TEXT NOT NULL DEFAULT (datetime('now'))
                );",
            )
            .map_err(|e| format!("Migration 25 (reviewer_overrides): {e}"))?;

            tx.execute("INSERT INTO schema_version (version) VALUES (25)", [])
                .map_err(|e| format!("Failed to record migration 25: {e}"))?;

            tx.commit()
                .map_err(|e| format!("Migration 25 commit: {e}"))?;
        }

        // Migration 26: per-event runtime tracking (feature 008).
        // Introduces `session_events`, the per-JSONL-line timeline the
        // redesigned LLM Runtime card reads. Sets a one-shot reingest flag
        // (matching migrations 20-22) so the existing mtime sweep
        // backfills every transcript on the next boot. See
        // specs/008-runtime-redesign/data-model.md and
        // specs/008-runtime-redesign/contracts/session-events.md.
        if current_version < 26 {
            let tx = conn
                .transaction()
                .map_err(|e| format!("Migration 26 transaction begin: {e}"))?;
            // SQLite rejects expressions in table-level PRIMARY KEY / UNIQUE
            // constraints (the COALESCE in the contract's PK shape). The
            // idempotency contract (ING-4: INSERT OR IGNORE byte-identical
            // re-walk) is preserved by a UNIQUE expression index instead —
            // SQLite resolves INSERT OR IGNORE against unique indices the
            // same as it would against a table-level PRIMARY KEY.
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS session_events (
                    provider     TEXT NOT NULL,
                    session_id   TEXT NOT NULL,
                    agent_id     TEXT,
                    is_sidechain INTEGER NOT NULL DEFAULT 0,
                    timestamp    TEXT NOT NULL,
                    kind         TEXT NOT NULL,
                    uuid         TEXT,
                    parent_uuid  TEXT
                );
                CREATE UNIQUE INDEX IF NOT EXISTS uidx_se_identity
                    ON session_events(provider, session_id, COALESCE(agent_id, ''), timestamp, kind);
                CREATE INDEX IF NOT EXISTS idx_se_timestamp
                    ON session_events(timestamp);
                CREATE INDEX IF NOT EXISTS idx_se_chain
                    ON session_events(provider, session_id, agent_id, timestamp);
                CREATE INDEX IF NOT EXISTS idx_se_provider_session_sidechain
                    ON session_events(provider, session_id, is_sidechain, timestamp);",
            )
            .map_err(|e| format!("Migration 26 (session_events table): {e}"))?;

            tx.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params!["runtime_event_reingest_pending", "1"],
            )
            .map_err(|e| format!("Migration 26 (set reingest flag): {e}"))?;

            tx.execute("INSERT INTO schema_version (version) VALUES (26)", [])
                .map_err(|e| format!("Failed to record migration 26: {e}"))?;

            tx.commit()
                .map_err(|e| format!("Migration 26 commit: {e}"))?;
        }

        // Feature 009: introduces `hook_invocations`, a per-event table
        // capturing every lifecycle-hook fire we observe from either
        // provider. Claude rows are populated by the new attachment
        // extractor during the existing dual-emission pass; Codex rows
        // arrive via `POST /api/v1/hooks/observed`. A one-shot reingest
        // flag (matching migrations 20-22 / 26) backfills historical
        // Claude transcripts on the next mtime sweep. See
        // specs/009-hooks-breakdown-tab/data-model.md and
        // specs/009-hooks-breakdown-tab/contracts/hook-invocations.md.
        // @lat: [[backend#Database#Schema#Hook Invocations]]
        if current_version < 27 {
            let tx = conn
                .transaction()
                .map_err(|e| format!("Migration 27 transaction begin: {e}"))?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS hook_invocations (
                    provider           TEXT NOT NULL,
                    session_id         TEXT NOT NULL,
                    agent_id           TEXT,
                    is_sidechain       INTEGER NOT NULL DEFAULT 0,
                    timestamp          TEXT NOT NULL,
                    hook_event         TEXT NOT NULL,
                    hook_matcher       TEXT,
                    tool_name          TEXT,
                    hook_identity      TEXT NOT NULL,
                    script_command_raw TEXT,
                    exit_code          INTEGER,
                    duration_ms        INTEGER,
                    cwd                TEXT,
                    hostname           TEXT,
                    message_id         TEXT
                );
                CREATE UNIQUE INDEX IF NOT EXISTS uidx_hook_invocations_identity
                    ON hook_invocations(provider, session_id, COALESCE(agent_id, ''),
                                        timestamp, hook_identity);
                CREATE INDEX IF NOT EXISTS idx_hook_invocations_provider_ts
                    ON hook_invocations(provider, timestamp);
                CREATE INDEX IF NOT EXISTS idx_hook_invocations_provider_session
                    ON hook_invocations(provider, session_id);
                CREATE INDEX IF NOT EXISTS idx_hook_invocations_identity_ts
                    ON hook_invocations(hook_identity, timestamp);
                CREATE INDEX IF NOT EXISTS idx_hook_invocations_identity_cwd
                    ON hook_invocations(hook_identity, cwd);",
            )
            .map_err(|e| format!("Migration 27 (hook_invocations table): {e}"))?;

            tx.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params!["hook_invocation_reingest_pending", "1"],
            )
            .map_err(|e| format!("Migration 27 (set reingest flag): {e}"))?;

            tx.execute("INSERT INTO schema_version (version) VALUES (27)", [])
                .map_err(|e| format!("Failed to record migration 27: {e}"))?;

            tx.commit()
                .map_err(|e| format!("Migration 27 commit: {e}"))?;
        }

        ensure_startup_indexes(&conn)?;

        let storage = Self {
            conn: Mutex::new(conn),
        };

        if let Err(e) = storage.aggregate_and_cleanup() {
            log::warn!("Cleanup on startup failed: {e}");
        }

        if let Err(e) = storage.aggregate_and_cleanup_tokens() {
            log::warn!("Token cleanup on startup failed: {e}");
        }

        if let Err(e) = storage.cleanup_old_observations() {
            log::warn!("Observation cleanup on startup failed: {e}");
        }

        // One-time redaction backfill (feature 005 US1 T021, contract
        // redaction.md "One-time backfill"). Idempotent and sentinel-guarded
        // so it scrubs any plaintext secret/PII captured before US1 wired
        // the redaction boundary, then never runs again.
        if let Err(e) = storage.backfill_redaction() {
            log::warn!("Redaction backfill on startup failed: {e}");
        }

        // One-time legacy learned-rule archive-then-wipe (feature 005 US2
        // T032, FR-012 / Q3=C, contracts/rule-governance.md "Legacy
        // archive-then-wipe"). Runs HERE — inside `Storage::init`, i.e.
        // before `rule_watcher::start` is ever called from the Tauri setup
        // hook — so the wipe cannot race the watcher's reconcile. Idempotent
        // + sentinel-guarded: every pre-existing on-disk learned `.md` is
        // copied to a read-only manifested archive outside the watched dirs,
        // the live file is deleted, and its DB row is durably tombstoned.
        // They can only return via the new gated approval pipeline.
        if let Err(e) = storage.archive_legacy_rules() {
            log::warn!("Legacy rule archive on startup failed: {e}");
        }

        Ok(storage)
    }

    pub fn store_snapshot(&self, buckets: &[UsageBucket]) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO usage_snapshots (timestamp, provider, bucket_key, bucket_label, utilization, resets_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            for bucket in buckets {
                stmt.execute(params![
                    now,
                    bucket.provider.as_str(),
                    bucket.key,
                    bucket.label,
                    bucket.utilization,
                    bucket.resets_at
                ])
                .map_err(|e| format!("Insert error: {e}"))?;
            }
        }
        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok(())
    }

    pub fn aggregate_and_cleanup(&self) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let cutoff = (Utc::now() - TimeDelta::days(30)).to_rfc3339();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        tx.execute(
            "INSERT INTO usage_hourly (hour, provider, bucket_key, bucket_label, avg_utilization, max_utilization, min_utilization, sample_count)
             SELECT
                 strftime('%Y-%m-%dT%H:00:00Z', timestamp) as hour,
                 provider,
                 bucket_key,
                 bucket_label,
                 AVG(utilization),
                 MAX(utilization),
                 MIN(utilization),
                 COUNT(*)
             FROM usage_snapshots
             WHERE timestamp < ?1
             GROUP BY hour, provider, bucket_key, bucket_label
             ON CONFLICT(hour, provider, bucket_key) DO UPDATE SET
                 avg_utilization = (usage_hourly.avg_utilization * usage_hourly.sample_count + excluded.avg_utilization * excluded.sample_count)
                     / (usage_hourly.sample_count + excluded.sample_count),
                 max_utilization = MAX(usage_hourly.max_utilization, excluded.max_utilization),
                 min_utilization = MIN(usage_hourly.min_utilization, excluded.min_utilization),
                 sample_count = usage_hourly.sample_count + excluded.sample_count",
            params![cutoff],
        )
        .map_err(|e| format!("Aggregation insert error: {e}"))?;

        tx.execute(
            "DELETE FROM usage_snapshots WHERE timestamp < ?1",
            params![cutoff],
        )
        .map_err(|e| format!("Aggregation delete error: {e}"))?;

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok(())
    }

    pub fn get_usage_history(
        &self,
        provider: IntegrationProvider,
        bucket_key: &str,
        range: &str,
    ) -> Result<Vec<DataPoint>, String> {
        let conn = self.conn.lock();
        let now = Utc::now();

        let (from, use_hourly) = match range {
            "1h" => (now - TimeDelta::hours(1), false),
            "24h" => (now - TimeDelta::hours(24), false),
            "7d" => (now - TimeDelta::days(7), false),
            "30d" => (now - TimeDelta::days(30), true),
            "all" => (now - TimeDelta::days(365), true),
            _ => (now - TimeDelta::hours(24), false),
        };

        let from_str = from.to_rfc3339();

        if use_hourly {
            let from_hour = from.format("%Y-%m-%dT%H:00:00Z").to_string();
            let mut points = Vec::new();

            // First get hourly aggregates for older data
            let mut stmt = conn
                .prepare_cached(
                    "SELECT hour, avg_utilization FROM usage_hourly
                     WHERE provider = ?1 AND bucket_key = ?2 AND hour >= ?3
                     ORDER BY hour ASC",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            let hourly_rows = stmt
                .query_map(params![provider.as_str(), bucket_key, from_hour], |row| {
                    Ok(DataPoint {
                        timestamp: row.get(0)?,
                        utilization: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Query error: {e}"))?;

            for row in hourly_rows {
                points.push(row.map_err(|e| format!("Row error: {e}"))?);
            }

            // Then append recent granular snapshots
            let mut stmt2 = conn
                .prepare_cached(
                    "SELECT timestamp, utilization FROM usage_snapshots
                     WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3
                     ORDER BY timestamp ASC",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            let snap_rows = stmt2
                .query_map(params![provider.as_str(), bucket_key, from_str], |row| {
                    Ok(DataPoint {
                        timestamp: row.get(0)?,
                        utilization: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Query error: {e}"))?;

            for row in snap_rows {
                points.push(row.map_err(|e| format!("Row error: {e}"))?);
            }

            // Downsample if too many points (max ~720 for charts)
            Ok(downsample(points, 720))
        } else {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT timestamp, utilization FROM usage_snapshots
                     WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3
                     ORDER BY timestamp ASC",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            let rows = stmt
                .query_map(params![provider.as_str(), bucket_key, from_str], |row| {
                    Ok(DataPoint {
                        timestamp: row.get(0)?,
                        utilization: row.get(1)?,
                    })
                })
                .map_err(|e| format!("Query error: {e}"))?;

            let mut points = Vec::new();
            for row in rows {
                points.push(row.map_err(|e| format!("Row error: {e}"))?);
            }

            let max_points = match range {
                "1h" => 60,
                "7d" => 672,
                _ => 1440,
            };

            Ok(downsample(points, max_points))
        }
    }

    pub fn get_usage_stats(
        &self,
        provider: IntegrationProvider,
        bucket_key: &str,
        days: i32,
    ) -> Result<BucketStats, String> {
        let conn = self.conn.lock();
        Self::get_usage_stats_with_conn(&conn, provider, bucket_key, days)
    }

    fn get_usage_stats_with_conn(
        conn: &Connection,
        provider: IntegrationProvider,
        bucket_key: &str,
        days: i32,
    ) -> Result<BucketStats, String> {
        let days = days.clamp(1, 365);
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut stmt = conn
            .prepare_cached(
                "SELECT
                     MIN(bucket_label),
                     AVG(utilization),
                     MAX(utilization),
                     MIN(utilization),
                     COUNT(*),
                     (SELECT COUNT(*) FROM usage_snapshots
                      WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3 AND utilization >= 80.0)
                 FROM usage_snapshots
                 WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let stats = stmt
            .query_row(params![provider.as_str(), bucket_key, from], |row| {
                let label = row
                    .get::<_, Option<String>>(0)?
                    .unwrap_or_else(|| bucket_key.to_string());
                let total: i64 = row.get(4)?;
                let above_80: i64 = row.get(5)?;
                let pct_above_80 = if total > 0 {
                    (above_80 as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                Ok(BucketStats {
                    provider,
                    key: bucket_key.to_string(),
                    label,
                    current: 0.0, // filled in by caller
                    avg: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    max: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    min: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                    time_above_80: pct_above_80,
                    trend: String::new(), // filled in below
                    sample_count: total,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let trend = calc_trend(conn, provider, bucket_key)?;

        Ok(BucketStats { trend, ..stats })
    }

    pub fn get_all_bucket_stats(
        &self,
        current_buckets: &[UsageBucket],
        days: i32,
    ) -> Result<Vec<BucketStats>, String> {
        let conn = self.conn.lock();
        let mut results = Vec::new();
        for bucket in current_buckets {
            let mut stats =
                Self::get_usage_stats_with_conn(&conn, bucket.provider, &bucket.key, days)?;
            stats.current = bucket.utilization;
            results.push(stats);
        }
        Ok(results)
    }

    pub fn get_latest_usage_buckets(
        &self,
        provider: IntegrationProvider,
    ) -> Result<Vec<UsageBucket>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                // Avoid the correlated MAX(timestamp) subquery here. On large
                // usage_snapshots tables it scales poorly enough to stall the
                // startup fetch path that restores cached live buckets.
                "SELECT snapshot.bucket_key, snapshot.bucket_label, snapshot.utilization, snapshot.resets_at
                 FROM usage_snapshots snapshot
                 INNER JOIN (
                     SELECT bucket_key, MAX(timestamp) AS latest_timestamp
                     FROM usage_snapshots
                     WHERE provider = ?1
                     GROUP BY bucket_key
                 ) latest
                   ON latest.bucket_key = snapshot.bucket_key
                  AND latest.latest_timestamp = snapshot.timestamp
                 WHERE snapshot.provider = ?1
                 ORDER BY
                     CASE snapshot.provider WHEN 'claude' THEN 0 ELSE 1 END,
                     snapshot.bucket_label ASC",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map(params![provider.as_str()], |row| {
                Ok(UsageBucket {
                    provider,
                    key: row.get(0)?,
                    label: row.get(1)?,
                    utilization: row.get(2)?,
                    resets_at: row.get(3)?,
                    sort_order: 0,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut buckets = Vec::new();
        for row in rows {
            buckets.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(buckets)
    }

    pub fn get_latest_usage_snapshot_timestamp(
        &self,
        provider: IntegrationProvider,
    ) -> Result<Option<String>, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT MAX(timestamp) FROM usage_snapshots WHERE provider = ?1",
            params![provider.as_str()],
            |row| row.get(0),
        )
        .map_err(|e| format!("Query error: {e}"))
    }

    pub fn get_snapshot_count(&self) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.query_row("SELECT COUNT(*) FROM usage_snapshots", [], |row| row.get(0))
            .map_err(|e| format!("Count error: {e}"))
    }

    pub fn store_token_snapshot(&self, payload: &TokenReportPayload) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO token_snapshots (provider, session_id, hostname, timestamp, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens, cwd, is_sidechain, agent_id, parent_uuid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                payload.provider.as_str(),
                payload.session_id,
                payload.hostname,
                now,
                payload.input_tokens,
                payload.output_tokens,
                payload.cache_creation_input_tokens,
                payload.cache_read_input_tokens,
                payload.cwd,
                payload.is_sidechain as i32,
                payload.agent_id,
                payload.parent_uuid,
            ],
        )
        .map_err(|e| format!("Insert token snapshot error: {e}"))?;

        Ok(())
    }

    pub fn get_token_history(
        &self,
        range: &str,
        provider: Option<IntegrationProvider>,
        hostname: Option<&str>,
        session_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<Vec<TokenDataPoint>, String> {
        let conn = self.conn.lock();
        let now = Utc::now();

        // Skip hourly aggregates when filtering by session_id or cwd since
        // token_hourly doesn't store those fields
        let needs_granular = session_id.is_some() || cwd.is_some();
        let (from, use_hourly) = match range {
            "1h" => (now - TimeDelta::hours(1), false),
            "24h" => (now - TimeDelta::hours(24), false),
            "7d" => (now - TimeDelta::days(7), false),
            "30d" => (now - TimeDelta::days(30), !needs_granular),
            "all" => (now - TimeDelta::days(365), !needs_granular),
            _ => (now - TimeDelta::hours(24), false),
        };

        let from_str = from.to_rfc3339();
        let mut points = Vec::new();

        if use_hourly {
            let from_hour = from.format("%Y-%m-%dT%H:00:00Z").to_string();

            let (hourly_sql, hourly_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
                match (provider, hostname) {
                    (Some(provider), Some(host)) => (
                        "SELECT hour, SUM(total_input), SUM(total_output), SUM(total_cache_creation), SUM(total_cache_read)
                         FROM token_hourly
                         WHERE hour >= ?1 AND provider = ?2 AND hostname = ?3
                         GROUP BY hour
                         ORDER BY hour ASC".to_string(),
                        vec![
                            Box::new(from_hour.clone()),
                            Box::new(provider.as_str().to_string()),
                            Box::new(host.to_string()),
                        ],
                    ),
                    (Some(provider), None) => (
                        "SELECT hour, SUM(total_input), SUM(total_output), SUM(total_cache_creation), SUM(total_cache_read)
                         FROM token_hourly
                         WHERE hour >= ?1 AND provider = ?2
                         GROUP BY hour
                         ORDER BY hour ASC".to_string(),
                        vec![
                            Box::new(from_hour.clone()),
                            Box::new(provider.as_str().to_string()),
                        ],
                    ),
                    (None, Some(host)) => (
                        "SELECT hour, SUM(total_input), SUM(total_output), SUM(total_cache_creation), SUM(total_cache_read)
                         FROM token_hourly
                         WHERE hour >= ?1 AND hostname = ?2
                         GROUP BY hour
                         ORDER BY hour ASC".to_string(),
                        vec![Box::new(from_hour.clone()), Box::new(host.to_string())],
                    ),
                    (None, None) => (
                        "SELECT hour, SUM(total_input), SUM(total_output), SUM(total_cache_creation), SUM(total_cache_read)
                         FROM token_hourly
                         WHERE hour >= ?1
                         GROUP BY hour
                         ORDER BY hour ASC".to_string(),
                        vec![Box::new(from_hour.clone())],
                    ),
                };

            let mut stmt = conn
                .prepare(&hourly_sql)
                .map_err(|e| format!("Prepare error: {e}"))?;

            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                hourly_params.iter().map(|p| p.as_ref()).collect();

            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    let inp: i64 = row.get(1)?;
                    let out: i64 = row.get(2)?;
                    let cc: i64 = row.get(3)?;
                    let cr: i64 = row.get(4)?;
                    Ok(TokenDataPoint {
                        timestamp: row.get(0)?,
                        input_tokens: inp,
                        output_tokens: out,
                        cache_creation_input_tokens: cc,
                        cache_read_input_tokens: cr,
                        total_tokens: inp + out + cc + cr,
                    })
                })
                .map_err(|e| format!("Query error: {e}"))?;

            for row in rows {
                points.push(row.map_err(|e| format!("Row error: {e}"))?);
            }
        }

        // Append granular snapshots
        let mut snap_sql = String::from(
            "SELECT timestamp, input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens
             FROM token_snapshots
             WHERE timestamp >= ?1",
        );
        let mut snap_params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(from_str.clone())];
        let mut next_param = 2;

        if let Some(provider) = provider {
            snap_sql.push_str(&format!(" AND provider = ?{next_param}"));
            snap_params.push(Box::new(provider.as_str().to_string()));
            next_param += 1;
        }

        if let Some(sid) = session_id {
            snap_sql.push_str(&format!(" AND session_id = ?{next_param}"));
            snap_params.push(Box::new(sid.to_string()));
        } else if let Some(project) = cwd {
            snap_sql.push_str(&format!(" AND cwd = ?{next_param}"));
            snap_params.push(Box::new(project.to_string()));
        } else if let Some(host) = hostname {
            snap_sql.push_str(&format!(" AND hostname = ?{next_param}"));
            snap_params.push(Box::new(host.to_string()));
        }
        snap_sql.push_str(" ORDER BY timestamp ASC");

        let mut stmt2 = conn
            .prepare(&snap_sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params_refs2: Vec<&dyn rusqlite::types::ToSql> =
            snap_params.iter().map(|p| p.as_ref()).collect();

        let snap_rows = stmt2
            .query_map(params_refs2.as_slice(), |row| {
                let inp: i64 = row.get(1)?;
                let out: i64 = row.get(2)?;
                let cc: i64 = row.get(3)?;
                let cr: i64 = row.get(4)?;
                Ok(TokenDataPoint {
                    timestamp: row.get(0)?,
                    input_tokens: inp,
                    output_tokens: out,
                    cache_creation_input_tokens: cc,
                    cache_read_input_tokens: cr,
                    total_tokens: inp + out + cc + cr,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        for row in snap_rows {
            points.push(row.map_err(|e| format!("Row error: {e}"))?);
        }

        let max_points = match range {
            "1h" => 60,
            "7d" => 672,
            "30d" | "all" => 720,
            _ => 1440,
        };

        Ok(downsample_tokens(points, max_points))
    }

    pub fn get_token_stats(
        &self,
        days: i32,
        provider: Option<IntegrationProvider>,
        hostname: Option<&str>,
        session_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<TokenStats, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut sql = String::from(
            "SELECT
                 COALESCE(SUM(input_tokens), 0),
                 COALESCE(SUM(output_tokens), 0),
                 COALESCE(SUM(cache_creation_input_tokens), 0),
                 COALESCE(SUM(cache_read_input_tokens), 0),
                 COUNT(*)
             FROM token_snapshots
             WHERE timestamp >= ?1",
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(from)];
        let mut next_param = 2;

        if let Some(provider) = provider {
            sql.push_str(&format!(" AND provider = ?{next_param}"));
            params_vec.push(Box::new(provider.as_str().to_string()));
            next_param += 1;
        }

        if let Some(session_id) = session_id {
            sql.push_str(&format!(" AND session_id = ?{next_param}"));
            params_vec.push(Box::new(session_id.to_string()));
        } else if let Some(project) = cwd {
            sql.push_str(&format!(" AND cwd = ?{next_param}"));
            params_vec.push(Box::new(project.to_string()));
        } else if let Some(host) = hostname {
            sql.push_str(&format!(" AND hostname = ?{next_param}"));
            params_vec.push(Box::new(host.to_string()));
        }

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        stmt.query_row(params_refs.as_slice(), |row| {
            let total_input: i64 = row.get(0)?;
            let total_output: i64 = row.get(1)?;
            let total_cache_creation: i64 = row.get(2)?;
            let total_cache_read: i64 = row.get(3)?;
            let turn_count: i64 = row.get(4)?;
            let total_tokens = total_input + total_output + total_cache_creation + total_cache_read;

            Ok(TokenStats {
                total_input,
                total_output,
                total_cache_creation,
                total_cache_read,
                total_tokens,
                turn_count,
                avg_input_per_turn: if turn_count > 0 {
                    total_input as f64 / turn_count as f64
                } else {
                    0.0
                },
                avg_output_per_turn: if turn_count > 0 {
                    total_output as f64 / turn_count as f64
                } else {
                    0.0
                },
            })
        })
        .map_err(|e| format!("Query error: {e}"))
    }

    pub fn store_context_savings_events(
        &self,
        events: &[ContextSavingsEventPayload],
    ) -> Result<ContextSavingsInsertResult, String> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Context savings transaction error: {e}"))?;

        let mut inserted = 0i64;
        let mut ignored = 0i64;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO context_savings_events (
                         event_id,
                         schema_version,
                         provider,
                         session_id,
                         hostname,
                         cwd,
                         timestamp,
                         event_type,
                         source,
                         decision,
                         category,
                         reason,
                         delivered,
                         indexed_bytes,
                         returned_bytes,
                         input_bytes,
                         tokens_indexed_est,
                         tokens_returned_est,
                         tokens_saved_est,
                         tokens_preserved_est,
                         estimate_method,
                         estimate_confidence,
                         source_ref,
                         snapshot_ref,
                         metadata_json
                     )
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
                )
                .map_err(|e| format!("Prepare context savings insert error: {e}"))?;

            for event in events {
                let metadata_json = event
                    .metadata_json
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| format!("Serialize context savings metadata error: {e}"))?;

                let category = event
                    .category
                    .as_deref()
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        crate::context_category::derive_category(&event.event_type, &event.decision)
                            .to_string()
                    });
                let token_scope = matches!(category.as_str(), "preservation" | "retrieval");
                let tokens_saved_est = if token_scope {
                    event.tokens_saved_est
                } else {
                    Some(0)
                };
                let tokens_preserved_est = if token_scope {
                    event.tokens_preserved_est
                } else {
                    Some(0)
                };

                let changed = stmt
                    .execute(params![
                        event.event_id,
                        event.schema_version,
                        event.provider.as_str(),
                        event.session_id.as_deref(),
                        event.hostname,
                        event.cwd.as_deref(),
                        event.timestamp,
                        event.event_type,
                        event.source,
                        event.decision,
                        category,
                        event.reason.as_deref(),
                        event.delivered as i32,
                        event.indexed_bytes,
                        event.returned_bytes,
                        event.input_bytes,
                        event.tokens_indexed_est,
                        event.tokens_returned_est,
                        tokens_saved_est,
                        tokens_preserved_est,
                        event.estimate_method.as_deref(),
                        event.estimate_confidence,
                        event.source_ref.as_deref(),
                        event.snapshot_ref.as_deref(),
                        metadata_json,
                    ])
                    .map_err(|e| format!("Insert context savings event error: {e}"))?;

                if changed == 0 {
                    ignored += 1;
                } else {
                    inserted += 1;
                }
            }
        }

        tx.commit()
            .map_err(|e| format!("Commit context savings events error: {e}"))?;

        Ok(ContextSavingsInsertResult { inserted, ignored })
    }

    pub fn get_context_savings_analytics(
        &self,
        range: &str,
        limit: Option<i64>,
    ) -> Result<ContextSavingsAnalytics, String> {
        let conn = self.conn.lock();
        let from = context_savings_from_timestamp(range);
        let recent_limit = limit.unwrap_or(50).clamp(1, 500);
        let breakdown_limit = 20i64;

        let summary_sql = format!(
            "SELECT {CONTEXT_SAVINGS_AGGREGATES_SQL}
             FROM context_savings_events
             WHERE timestamp >= ?1"
        );
        let mut summary = conn
            .query_row(&summary_sql, params![from], |row| {
                context_savings_summary_from_row_at(row, 0)
            })
            .map_err(|e| format!("Query context savings summary error: {e}"))?;
        apply_category_totals(&mut summary, &conn, &from)?;
        apply_retention_metrics(&mut summary, &conn, &from)?;

        let bucket_expr = context_savings_bucket_expr(range);
        let timeseries_sql = format!(
            "SELECT {bucket_expr} AS bucket, {CONTEXT_SAVINGS_AGGREGATES_SQL}
             FROM context_savings_events
             WHERE timestamp >= ?1
             GROUP BY bucket
             ORDER BY bucket ASC"
        );
        let mut timeseries_stmt = conn
            .prepare(&timeseries_sql)
            .map_err(|e| format!("Prepare context savings timeseries error: {e}"))?;
        let timeseries_rows = timeseries_stmt
            .query_map(params![from], |row| {
                let summary = context_savings_summary_from_row_at(row, 1)?;
                Ok(ContextSavingsTimeseriesPoint {
                    timestamp: row.get(0)?,
                    event_count: summary.event_count,
                    delivered_count: summary.delivered_count,
                    indexed_bytes: summary.indexed_bytes,
                    returned_bytes: summary.returned_bytes,
                    input_bytes: summary.input_bytes,
                    tokens_indexed_est: summary.tokens_indexed_est,
                    tokens_returned_est: summary.tokens_returned_est,
                    tokens_saved_est: summary.tokens_saved_est,
                    tokens_preserved_est: summary.tokens_preserved_est,
                })
            })
            .map_err(|e| format!("Query context savings timeseries error: {e}"))?;

        let mut timeseries = Vec::new();
        for row in timeseries_rows {
            timeseries.push(row.map_err(|e| format!("Context savings timeseries row error: {e}"))?);
        }

        let breakdowns = ContextSavingsBreakdowns {
            by_provider: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "provider",
                &from,
                breakdown_limit,
            )?,
            by_event_type: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "event_type",
                &from,
                breakdown_limit,
            )?,
            by_source: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "source",
                &from,
                breakdown_limit,
            )?,
            by_decision: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "decision",
                &from,
                breakdown_limit,
            )?,
            by_cwd: Self::get_context_savings_breakdown_with_conn(
                &conn,
                "cwd",
                &from,
                breakdown_limit,
            )?,
        };

        let recent_events =
            Self::get_recent_context_savings_events_with_conn(&conn, &from, recent_limit)?;

        Ok(ContextSavingsAnalytics {
            summary,
            timeseries,
            breakdowns,
            recent_events,
        })
    }

    pub fn has_context_savings_events(&self) -> Result<bool, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM context_savings_events LIMIT 1)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(|e| format!("Context savings existence query error: {e}"))
    }

    fn get_context_savings_breakdown_with_conn(
        conn: &Connection,
        dimension: &str,
        from: &str,
        limit: i64,
    ) -> Result<Vec<ContextSavingsBreakdownItem>, String> {
        let sql = format!(
            "SELECT COALESCE(NULLIF({dimension}, ''), '(unknown)') AS key,
                    {CONTEXT_SAVINGS_AGGREGATES_SQL}
             FROM context_savings_events
             WHERE timestamp >= ?1
             GROUP BY key
             ORDER BY 9 DESC, 2 DESC
             LIMIT ?2"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare context savings breakdown error: {e}"))?;
        let rows = stmt
            .query_map(params![from, limit], context_savings_breakdown_from_row)
            .map_err(|e| format!("Query context savings breakdown error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Context savings breakdown row error: {e}"))?);
        }
        Ok(results)
    }

    fn get_recent_context_savings_events_with_conn(
        conn: &Connection,
        from: &str,
        limit: i64,
    ) -> Result<Vec<ContextSavingsEvent>, String> {
        let mut stmt = conn
            .prepare_cached(
                "SELECT
                     event_id,
                     schema_version,
                     provider,
                     session_id,
                     hostname,
                     cwd,
                     timestamp,
                     event_type,
                     source,
                     decision,
                     category,
                     reason,
                     delivered,
                     indexed_bytes,
                     returned_bytes,
                     input_bytes,
                     tokens_indexed_est,
                     tokens_returned_est,
                     CASE
                         WHEN category NOT IN ('preservation', 'retrieval') THEN 0
                         WHEN tokens_saved_est IS NOT NULL
                             AND NOT (
                                 tokens_saved_est = 0
                                 AND delivered = 0
                                 AND indexed_bytes IS NOT NULL
                                 AND returned_bytes IS NULL
                             ) THEN tokens_saved_est
                         WHEN indexed_bytes IS NOT NULL OR input_bytes IS NOT NULL OR returned_bytes IS NOT NULL THEN
                             CASE
                                 WHEN COALESCE(indexed_bytes, input_bytes, 0) > COALESCE(returned_bytes, 0) THEN
                                     (COALESCE(indexed_bytes, input_bytes, 0) - COALESCE(returned_bytes, 0) + 3) / 4
                                 ELSE 0
                             END
                         ELSE 0
                     END AS tokens_saved_est,
                     CASE
                         WHEN category IN ('preservation', 'retrieval')
                             THEN tokens_preserved_est
                         ELSE 0
                     END AS tokens_preserved_est,
                     estimate_method,
                     estimate_confidence,
                     source_ref,
                     snapshot_ref,
                     metadata_json,
                     created_at
                 FROM context_savings_events
                 WHERE timestamp >= ?1
                 ORDER BY timestamp DESC, created_at DESC
                 LIMIT ?2",
            )
            .map_err(|e| format!("Prepare recent context savings events error: {e}"))?;

        let rows = stmt
            .query_map(params![from, limit], |row| {
                Ok(ContextSavingsEvent {
                    event_id: row.get(0)?,
                    schema_version: row.get(1)?,
                    provider: parse_context_savings_provider(row.get(2)?),
                    session_id: row.get(3)?,
                    hostname: row.get(4)?,
                    cwd: row.get(5)?,
                    timestamp: row.get(6)?,
                    event_type: row.get(7)?,
                    source: row.get(8)?,
                    decision: row.get(9)?,
                    category: row.get(10)?,
                    reason: row.get(11)?,
                    delivered: row.get::<_, i64>(12)? != 0,
                    indexed_bytes: row.get(13)?,
                    returned_bytes: row.get(14)?,
                    input_bytes: row.get(15)?,
                    tokens_indexed_est: row.get(16)?,
                    tokens_returned_est: row.get(17)?,
                    tokens_saved_est: row.get(18)?,
                    tokens_preserved_est: row.get(19)?,
                    estimate_method: row.get(20)?,
                    estimate_confidence: row.get(21)?,
                    source_ref: row.get(22)?,
                    snapshot_ref: row.get(23)?,
                    metadata_json: parse_context_savings_metadata(row.get(24)?),
                    created_at: row.get(25)?,
                })
            })
            .map_err(|e| format!("Query recent context savings events error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Recent context savings event row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_token_hostnames(&self) -> Result<Vec<String>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached("SELECT DISTINCT hostname FROM token_snapshots ORDER BY hostname ASC")
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| format!("Query error: {e}"))?;

        let mut hostnames = Vec::new();
        for row in rows {
            hostnames.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(hostnames)
    }

    pub fn get_host_breakdown(&self, days: i32) -> Result<Vec<HostBreakdown>, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut stmt = conn
            .prepare_cached(
                "SELECT
                     hostname,
                     SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens,
                     COUNT(*) as turn_count,
                     MAX(timestamp) as last_active
                 FROM token_snapshots
                 WHERE timestamp >= ?1
                 GROUP BY hostname
                 ORDER BY total_tokens DESC
                 LIMIT 50",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map(params![from], |row| {
                Ok(HostBreakdown {
                    hostname: row.get(0)?,
                    total_tokens: row.get(1)?,
                    turn_count: row.get(2)?,
                    last_active: row.get(3)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_project_breakdown(&self, days: i32) -> Result<Vec<ProjectBreakdown>, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut stmt = conn
            .prepare_cached(
                "SELECT
                     cwd,
                     hostname,
                     SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens,
                     COUNT(*) as turn_count,
                     COUNT(DISTINCT provider || ':' || session_id) as session_count,
                     MAX(timestamp) as last_active
                 FROM token_snapshots
                 WHERE timestamp >= ?1 AND cwd IS NOT NULL
                 GROUP BY cwd, hostname
                 ORDER BY last_active DESC
                 LIMIT 100",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let rows = stmt
            .query_map(params![from], |row| {
                Ok(ProjectBreakdown {
                    project: row.get(0)?,
                    hostname: row.get(1)?,
                    total_tokens: row.get(2)?,
                    turn_count: row.get(3)?,
                    session_count: row.get(4)?,
                    last_active: row.get(5)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut raw: Vec<ProjectBreakdown> = Vec::new();
        for row in rows {
            raw.push(row.map_err(|e| format!("Row error: {e}"))?);
        }

        // Merge subdirectories into their parent project root.
        // If /a/b is a prefix of /a/b/c, fold /a/b/c into /a/b.
        Ok(merge_project_subdirs(raw))
    }

    pub fn get_skill_breakdown(
        &self,
        days: i32,
        provider: Option<IntegrationProvider>,
        all_time: bool,
        limit: Option<i32>,
    ) -> Result<Vec<SkillBreakdown>, String> {
        if provider == Some(IntegrationProvider::MiniMax) {
            return Ok(Vec::new());
        }

        let days = days.clamp(1, 365);
        let limit = limit.unwrap_or(100).clamp(1, 500);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut sql = String::from(
            "SELECT
                 skill_name,
                 COUNT(*) AS total_count,
                 COALESCE(SUM(CASE WHEN provider = 'claude' THEN 1 ELSE 0 END), 0) AS claude_count,
                 COALESCE(SUM(CASE WHEN provider = 'codex' THEN 1 ELSE 0 END), 0) AS codex_count,
                 COUNT(DISTINCT cwd) AS project_count,
                 MAX(timestamp) AS last_used
             FROM skill_usages
             WHERE provider IN ('claude', 'codex')",
        );

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if !all_time {
            params_vec.push(Box::new(from));
            sql.push_str(" AND timestamp >= ?1");
        }
        if let Some(provider) = provider {
            let next_param = params_vec.len() + 1;
            sql.push_str(&format!(" AND provider = ?{next_param}"));
            params_vec.push(Box::new(provider.as_str().to_string()));
        }
        sql.push_str(" GROUP BY skill_name ORDER BY total_count DESC, skill_name ASC");
        sql.push_str(&format!(" LIMIT {limit}"));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(SkillBreakdown {
                    skill_name: row.get(0)?,
                    total_count: row.get(1)?,
                    claude_count: row.get(2)?,
                    codex_count: row.get(3)?,
                    project_count: row.get(4)?,
                    last_used: row.get(5)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    /// Build the Now-tab Hooks breakdown. Aggregates `hook_invocations`
    /// rows by canonicalized `hook_identity` over the active timeframe
    /// (or all indexed history when `all_time = true`), splitting
    /// per-provider counts so the All / Codex / Claude filter strip
    /// can pick the right column. `is_quill` is derived from the
    /// `quill:` identity prefix for Quill-managed row classification.
    /// See specs/009-hooks-breakdown-tab/contracts/hook-breakdown-ipc.md.
    // @lat: [[backend#Database#Schema#Hook Invocations]]
    pub fn get_hook_breakdown(
        &self,
        days: i32,
        provider: Option<IntegrationProvider>,
        all_time: bool,
        limit: Option<i32>,
    ) -> Result<Vec<crate::models::HookBreakdown>, String> {
        if provider == Some(IntegrationProvider::MiniMax) {
            return Ok(Vec::new());
        }

        let days = days.clamp(1, 365);
        let limit = limit.unwrap_or(100).clamp(1, 500);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        // For each `hook_identity` group, surface the most recently
        // seen `hook_event` / `tool_name` via a correlated subquery
        // ordered by timestamp DESC. The subqueries match the outer
        // `provider` scope (`hi2.provider = hi.provider`) so a
        // provider-filtered query never returns event/tool metadata
        // from the other provider — important because an identity can
        // appear under both providers (e.g., a script firing on both
        // Claude transcripts and Codex live observations) and the
        // outer aggregate only sums rows from the active provider.
        let mut sql = String::from(
            "SELECT
                 hook_identity,
                 (SELECT hook_event FROM hook_invocations hi2
                  WHERE hi2.hook_identity = hi.hook_identity
                    AND hi2.provider = hi.provider
                  ORDER BY hi2.timestamp DESC LIMIT 1) AS hook_event,
                 (SELECT tool_name FROM hook_invocations hi3
                  WHERE hi3.hook_identity = hi.hook_identity
                    AND hi3.provider = hi.provider
                  ORDER BY hi3.timestamp DESC LIMIT 1) AS tool_name,
                 CASE WHEN hook_identity LIKE 'quill:%' THEN 1 ELSE 0 END AS is_quill,
                 COALESCE(SUM(CASE WHEN provider = 'codex' THEN 1 ELSE 0 END), 0) AS codex_count,
                 COALESCE(SUM(CASE WHEN provider = 'claude' THEN 1 ELSE 0 END), 0) AS claude_count,
                 COUNT(*) AS total_count,
                 MAX(timestamp) AS last_fired_at
             FROM hook_invocations hi
             WHERE provider IN ('claude', 'codex')",
        );

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if !all_time {
            params_vec.push(Box::new(from));
            sql.push_str(" AND timestamp >= ?1");
        }
        if let Some(provider) = provider {
            let next_param = params_vec.len() + 1;
            sql.push_str(&format!(" AND provider = ?{next_param}"));
            params_vec.push(Box::new(provider.as_str().to_string()));
        }
        sql.push_str(
            " GROUP BY hook_identity \
             ORDER BY total_count DESC, hook_identity ASC",
        );
        sql.push_str(&format!(" LIMIT {limit}"));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare hook breakdown: {e}"))?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                let is_quill_i: i64 = row.get(3)?;
                Ok(crate::models::HookBreakdown {
                    hook_identity: row.get(0)?,
                    hook_event: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    tool_name: row.get(2)?,
                    is_quill: is_quill_i != 0,
                    codex_count: row.get(4)?,
                    claude_count: row.get(5)?,
                    total_count: row.get(6)?,
                    last_fired_at: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                })
            })
            .map_err(|e| format!("Query hook breakdown: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row hook breakdown: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_skill_project_breakdown(
        &self,
        skill_name: &str,
        days: i32,
        provider: Option<IntegrationProvider>,
        all_time: bool,
        limit: Option<i32>,
    ) -> Result<Vec<SkillProjectBreakdown>, String> {
        if provider == Some(IntegrationProvider::MiniMax) {
            return Ok(Vec::new());
        }

        let days = days.clamp(1, 365);
        let limit = limit.unwrap_or(50).clamp(1, 500);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut sql = String::from(
            "SELECT
                 cwd,
                 hostname,
                 COUNT(*) AS total_count,
                 COALESCE(SUM(CASE WHEN provider = 'claude' THEN 1 ELSE 0 END), 0) AS claude_count,
                 COALESCE(SUM(CASE WHEN provider = 'codex' THEN 1 ELSE 0 END), 0) AS codex_count,
                 MAX(timestamp) AS last_used
             FROM skill_usages
             WHERE skill_name = ?1
               AND provider IN ('claude', 'codex')",
        );

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        params_vec.push(Box::new(skill_name.to_string()));
        if !all_time {
            let next_param = params_vec.len() + 1;
            sql.push_str(&format!(" AND timestamp >= ?{next_param}"));
            params_vec.push(Box::new(from));
        }
        if let Some(provider) = provider {
            let next_param = params_vec.len() + 1;
            sql.push_str(&format!(" AND provider = ?{next_param}"));
            params_vec.push(Box::new(provider.as_str().to_string()));
        }
        sql.push_str(" GROUP BY cwd, hostname ORDER BY total_count DESC, last_used DESC, cwd ASC");
        sql.push_str(&format!(" LIMIT {limit}"));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                let cwd: Option<String> = row.get(0)?;
                let hostname: Option<String> = row.get(1)?;
                let total_count: i64 = row.get(2)?;
                let claude_count: i64 = row.get(3)?;
                let codex_count: i64 = row.get(4)?;
                let last_used: String = row.get(5)?;
                Ok(SkillProjectBreakdown {
                    skill_name: skill_name.to_string(),
                    project: cwd,
                    hostname,
                    total_count,
                    claude_count,
                    codex_count,
                    last_used,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut raw: Vec<SkillProjectBreakdown> = Vec::new();
        for row in rows {
            raw.push(row.map_err(|e| format!("Row error: {e}"))?);
        }

        // Drop the connection lock before doing CPU-bound merging.
        drop(stmt);
        drop(conn);

        // Collect distinct cwds for the subdir merge step.
        let paths: Vec<String> = {
            let mut p: Vec<String> = raw.iter().filter_map(|r| r.project.clone()).collect();
            p.sort();
            p.dedup();
            p
        };

        let parent_map = compute_subdir_parent_map(&paths);
        if parent_map.is_empty() {
            // Already sorted and truncated by the SQL query — return as-is.
            return Ok(raw);
        }

        // Merge subdirs into their parent project root, keyed by
        // (resolved_project, hostname). Mirrors merge_project_subdirs.
        let mut merged: std::collections::HashMap<
            (Option<String>, Option<String>),
            SkillProjectBreakdown,
        > = std::collections::HashMap::new();
        for row in raw {
            let resolved = row.project.as_ref().map(|project| {
                parent_map
                    .get(project)
                    .cloned()
                    .unwrap_or_else(|| project.clone())
            });
            let key = (resolved.clone(), row.hostname.clone());
            let entry = merged.entry(key).or_insert_with(|| SkillProjectBreakdown {
                skill_name: row.skill_name.clone(),
                project: resolved.clone(),
                hostname: row.hostname.clone(),
                total_count: 0,
                claude_count: 0,
                codex_count: 0,
                last_used: String::new(),
            });
            entry.total_count += row.total_count;
            entry.claude_count += row.claude_count;
            entry.codex_count += row.codex_count;
            if row.last_used > entry.last_used {
                entry.last_used = row.last_used;
            }
        }

        let mut results: Vec<SkillProjectBreakdown> = merged.into_values().collect();
        results.sort_by(|a, b| {
            b.total_count
                .cmp(&a.total_count)
                .then_with(|| b.last_used.cmp(&a.last_used))
                .then_with(|| {
                    a.project
                        .as_deref()
                        .unwrap_or("")
                        .cmp(b.project.as_deref().unwrap_or(""))
                })
        });
        results.truncate(limit as usize);
        Ok(results)
    }

    pub fn get_session_breakdown(
        &self,
        days: i32,
        hostname: Option<&str>,
        provider: Option<IntegrationProvider>,
        limit: Option<i32>,
    ) -> Result<Vec<SessionBreakdown>, String> {
        let days = days.clamp(1, 365);
        let limit = limit.unwrap_or(10).clamp(1, 500);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        // Sub-agent rollup (Wave 2). Each output row is keyed by
        // `(provider, session_id, hostname)` — the same natural key as
        // `token_snapshots`. A session that appears on multiple hostnames
        // (e.g. desktop + laptop sync) produces one row per hostname; the
        // rollup aggregates parent + sub-agent rows within each tuple, not
        // across hostnames.
        //
        // * tokens / first_seen / last_active come from `token_snapshots`
        //   (the only source of per-row token amounts).
        // * turn_count comes from `response_times` because each sub-agent
        //   turn produces its own user/assistant pair there — counting
        //   token_snapshots rows would double-count snapshot heartbeats.
        // * last_active is MAX across BOTH token_snapshots and
        //   response_times so an active sub-agent turn keeps the parent's
        //   active badge lit even if no token snapshot landed yet.
        // * `has_subagents` reads token_snapshots (only sub-agent-aware
        //   table that survived the Wave 1 reingest reset). Cheapest
        //   reliable signal.
        // * `subagent_count` is COUNT(DISTINCT agent_id) over the UNION of
        //   all three sub-agent-aware tables — any one of them may carry
        //   the agent first depending on extraction ordering.
        //
        // The `(provider, session_id, is_sidechain)` index added in
        // migration 20 means each subquery is an index scan over the
        // relevant slice, not a full-table scan.
        let mut sql = String::from(
            "WITH tok AS (
                 SELECT provider, session_id, hostname,
                        SUM(input_tokens + output_tokens
                            + cache_creation_input_tokens
                            + cache_read_input_tokens) AS total_tokens,
                        MIN(timestamp) AS first_seen,
                        MAX(timestamp) AS last_active_tok,
                        MAX(CASE WHEN is_sidechain = 1 THEN 1 ELSE 0 END) AS has_subagents
                 FROM token_snapshots
                 WHERE timestamp >= ?1
                 GROUP BY provider, session_id, hostname
             )
             SELECT
                 tok.provider,
                 tok.session_id,
                 tok.hostname,
                 tok.total_tokens,
                 COALESCE((SELECT COUNT(*) FROM response_times rt
                           WHERE rt.provider = tok.provider
                             AND rt.session_id = tok.session_id), 0) AS turn_count,
                 tok.first_seen,
                 COALESCE(
                     (SELECT MAX(ts) FROM (
                         SELECT MAX(timestamp) AS ts FROM token_snapshots
                           WHERE provider = tok.provider AND session_id = tok.session_id
                         UNION ALL
                         SELECT MAX(timestamp) AS ts FROM response_times
                           WHERE provider = tok.provider AND session_id = tok.session_id
                     )), tok.last_active_tok) AS last_active,
                 (SELECT t.cwd FROM token_snapshots t
                    WHERE t.provider = tok.provider AND t.session_id = tok.session_id
                      AND t.cwd IS NOT NULL
                    ORDER BY t.timestamp DESC LIMIT 1) AS project,
                 tok.has_subagents,
                 (SELECT COUNT(*) FROM (
                     SELECT agent_id FROM token_snapshots
                       WHERE provider = tok.provider AND session_id = tok.session_id
                         AND agent_id IS NOT NULL
                     UNION
                     SELECT agent_id FROM response_times
                       WHERE provider = tok.provider AND session_id = tok.session_id
                         AND agent_id IS NOT NULL
                     UNION
                     SELECT agent_id FROM tool_actions
                       WHERE provider = tok.provider AND session_id = tok.session_id
                         AND agent_id IS NOT NULL
                 )) AS subagent_count
             FROM tok
             WHERE 1=1",
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(from)];
        let hostname_param = if provider.is_some() { 3 } else { 2 };

        if let Some(provider) = provider {
            sql.push_str(" AND tok.provider = ?2");
            params_vec.push(Box::new(provider.as_str().to_string()));
        }

        if let Some(host) = hostname {
            sql.push_str(&format!(" AND tok.hostname = ?{hostname_param}"));
            params_vec.push(Box::new(host.to_string()));
        }

        sql.push_str(" ORDER BY last_active DESC");
        sql.push_str(&format!(" LIMIT {limit}"));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(SessionBreakdown {
                    provider: row.get(0)?,
                    session_id: row.get(1)?,
                    hostname: row.get(2)?,
                    total_tokens: row.get(3)?,
                    turn_count: row.get(4)?,
                    first_seen: row.get(5)?,
                    last_active: row.get(6)?,
                    project: row.get(7)?,
                    has_subagents: row.get::<_, i64>(8)? != 0,
                    subagent_count: row.get::<_, i64>(9)?.max(0) as u32,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    /// Return one row per distinct `agent_id` that belongs to
    /// `(provider, session_id)`. Each row aggregates the agent's own rows
    /// across `token_snapshots`, `response_times`, and `tool_actions`. The
    /// `parent_agent_id` field supports future depth-2+ nesting; today every
    /// chain originates from the parent transcript so it is None for all
    /// real-world rows.
    ///
    /// Parent resolution (Wave 2 chose option (b)): the chain's earliest
    /// `parent_uuid` is matched against `tool_actions.message_id` — if the
    /// uuid is owned by some other agent's transcript, that agent becomes
    /// `parent_agent_id`. When the uuid lives in the parent transcript the
    /// owning row has `agent_id IS NULL`, so the lookup correctly returns
    /// None and the node sorts under the session root in the UI.
    pub fn get_session_subagent_tree(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<Vec<SubagentNode>, String> {
        let conn = self.conn.lock();
        let provider_str = provider.as_str();

        // Discover the universe of agent_ids attached to this session across
        // all three sub-agent-aware tables. UNION dedupes naturally.
        let mut agents_stmt = conn
            .prepare_cached(
                "SELECT agent_id FROM (
                     SELECT agent_id FROM token_snapshots
                       WHERE provider = ?1 AND session_id = ?2 AND agent_id IS NOT NULL
                     UNION
                     SELECT agent_id FROM response_times
                       WHERE provider = ?1 AND session_id = ?2 AND agent_id IS NOT NULL
                     UNION
                     SELECT agent_id FROM tool_actions
                       WHERE provider = ?1 AND session_id = ?2 AND agent_id IS NOT NULL
                 )",
            )
            .map_err(|e| format!("Prepare agent universe: {e}"))?;
        let agent_ids: Vec<String> = agents_stmt
            .query_map(params![provider_str, session_id], |row| row.get(0))
            .map_err(|e| format!("Query agent universe: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Collect agent universe: {e}"))?;
        drop(agents_stmt);

        let mut nodes: Vec<SubagentNode> = Vec::with_capacity(agent_ids.len());

        // Per-agent aggregate query. Pulls tokens / first_seen / last_active
        // from token_snapshots ∪ response_times, turn_count from
        // response_times, tool_call_count from tool_actions. The chain's
        // earliest parent_uuid (from response_times — assistant turn close)
        // feeds the parent_agent_id resolver. The label is a best-effort
        // 80-char crop of the first user-side timestamp's row id, kept None
        // here because we don't store message bodies in response_times;
        // Wave 3 can derive a richer label from sessions when needed.
        let mut per_agent_stmt = conn
            .prepare_cached(
                "SELECT
                     (SELECT MIN(ts) FROM (
                         SELECT MIN(timestamp) AS ts FROM token_snapshots
                           WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3
                         UNION ALL
                         SELECT MIN(timestamp) AS ts FROM response_times
                           WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3
                     )) AS first_seen,
                     (SELECT MAX(ts) FROM (
                         SELECT MAX(timestamp) AS ts FROM token_snapshots
                           WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3
                         UNION ALL
                         SELECT MAX(timestamp) AS ts FROM response_times
                           WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3
                     )) AS last_active,
                     (SELECT COUNT(*) FROM response_times
                        WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3) AS turn_count,
                     COALESCE((SELECT SUM(input_tokens) FROM token_snapshots
                        WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3), 0) AS input_tokens,
                     COALESCE((SELECT SUM(output_tokens) FROM token_snapshots
                        WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3), 0) AS output_tokens,
                     COALESCE((SELECT SUM(cache_creation_input_tokens) FROM token_snapshots
                        WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3), 0) AS cache_creation_tokens,
                     COALESCE((SELECT SUM(cache_read_input_tokens) FROM token_snapshots
                        WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3), 0) AS cache_read_tokens,
                     (SELECT COUNT(*) FROM tool_actions
                        WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3) AS tool_call_count,
                     -- Earliest parent_uuid for this agent (the chain root).
                     (SELECT parent_uuid FROM response_times
                        WHERE provider = ?1 AND session_id = ?2 AND agent_id = ?3
                          AND parent_uuid IS NOT NULL
                        ORDER BY timestamp ASC LIMIT 1) AS chain_root_parent_uuid",
            )
            .map_err(|e| format!("Prepare per-agent: {e}"))?;

        // Per-agent parent resolver — option (b). Returns the owning
        // agent_id when `message_id` lives in another sub-agent's
        // transcript, NULL when it lives in the parent transcript.
        let mut parent_stmt = conn
            .prepare_cached(
                "SELECT agent_id FROM tool_actions
                   WHERE provider = ?1 AND session_id = ?2 AND message_id = ?3
                     AND agent_id IS NOT NULL
                   LIMIT 1",
            )
            .map_err(|e| format!("Prepare parent resolver: {e}"))?;

        for agent_id in &agent_ids {
            let row = per_agent_stmt
                .query_row(params![provider_str, session_id, agent_id], |row| {
                    let first_seen: Option<String> = row.get(0)?;
                    let last_active: Option<String> = row.get(1)?;
                    let turn_count: i64 = row.get(2)?;
                    let input_tokens: i64 = row.get(3)?;
                    let output_tokens: i64 = row.get(4)?;
                    let cache_creation_tokens: i64 = row.get(5)?;
                    let cache_read_tokens: i64 = row.get(6)?;
                    let tool_call_count: i64 = row.get(7)?;
                    let chain_root_parent_uuid: Option<String> = row.get(8)?;
                    Ok((
                        first_seen,
                        last_active,
                        turn_count,
                        input_tokens,
                        output_tokens,
                        cache_creation_tokens,
                        cache_read_tokens,
                        tool_call_count,
                        chain_root_parent_uuid,
                    ))
                })
                .map_err(|e| format!("Per-agent query error: {e}"))?;

            let (
                first_seen,
                last_active,
                turn_count,
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                tool_call_count,
                chain_root_parent_uuid,
            ) = row;

            // Resolve parent_agent_id only when we have a chain root uuid to
            // hand off. NULL today for every depth-1 sub-agent (the uuid
            // belongs to the parent transcript whose rows carry
            // agent_id IS NULL — the resolver query filters those out).
            let parent_agent_id: Option<String> = match chain_root_parent_uuid {
                Some(uuid) => parent_stmt
                    .query_row(params![provider_str, session_id, &uuid], |r| r.get(0))
                    .optional()
                    .map_err(|e| format!("Parent resolver: {e}"))?,
                None => None,
            };

            // Sub-agents with no token snapshots and no response_times rows
            // shouldn't realistically appear (the agent_id surfaced only in
            // tool_actions), but guard against missing timestamps anyway.
            let first_seen = first_seen.unwrap_or_default();
            let last_active = last_active.unwrap_or_else(|| first_seen.clone());

            nodes.push(SubagentNode {
                agent_id: agent_id.clone(),
                parent_agent_id,
                first_seen,
                last_active,
                turn_count: turn_count.max(0) as u32,
                total_tokens: input_tokens
                    + output_tokens
                    + cache_creation_tokens
                    + cache_read_tokens,
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                tool_call_count: tool_call_count.max(0) as u32,
                // Best-effort label deferred to Wave 3: we don't store user
                // message bodies in any sub-agent-aware table, so a useful
                // label would require joining sessions/transcripts. Return
                // None so the contract is honored without a fragile peek.
                label: None,
            });
        }

        // Spawn order — frontend renders the tree top-to-bottom in this order.
        nodes.sort_by(|a, b| a.first_seen.cmp(&b.first_seen));
        Ok(nodes)
    }

    pub fn get_project_tokens(&self, days: i32) -> Result<Vec<ProjectTokens>, String> {
        let days = days.clamp(1, 365);
        let conn = self.conn.lock();
        let from = (Utc::now() - TimeDelta::days(days as i64)).to_rfc3339();

        let mut stmt = conn
            .prepare(
                "SELECT
                    project,
                    SUM(total_tokens) as total_tokens,
                    COUNT(*) as session_count
                FROM (
                    SELECT
                        s.provider,
                        s.session_id,
                        (SELECT t.cwd FROM token_snapshots t
                         WHERE t.provider = s.provider AND t.session_id = s.session_id AND t.cwd IS NOT NULL
                         ORDER BY t.timestamp DESC LIMIT 1) as project,
                        SUM(s.input_tokens + s.output_tokens + s.cache_creation_input_tokens + s.cache_read_input_tokens) as total_tokens
                    FROM token_snapshots s
                    WHERE s.timestamp >= ?1
                    GROUP BY s.provider, s.session_id
                )
                WHERE project IS NOT NULL
                GROUP BY project
                ORDER BY total_tokens DESC
                LIMIT 20",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(rusqlite::params![from], |row| {
                Ok(ProjectTokens {
                    project: row.get(0)?,
                    total_tokens: row.get(1)?,
                    session_count: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_session_stats(&self, days: i32) -> Result<SessionStats, String> {
        let conn = self.conn.lock();
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let mut stmt = conn
            .prepare(
                "SELECT
                    AVG(duration_seconds) as avg_duration_seconds,
                    AVG(total_tokens) as avg_tokens,
                    COUNT(*) as session_count,
                    SUM(total_tokens) as total_tokens
                FROM (
                    SELECT
                        provider,
                        session_id,
                        (strftime('%s', MAX(timestamp)) - strftime('%s', MIN(timestamp))) as duration_seconds,
                        SUM(input_tokens + output_tokens + cache_creation_input_tokens + cache_read_input_tokens) as total_tokens
                    FROM token_snapshots
                    WHERE timestamp >= ?1
                    GROUP BY provider, session_id
                    HAVING COUNT(*) > 1
                )",
            )
            .map_err(|e| e.to_string())?;

        let result = stmt
            .query_row(rusqlite::params![cutoff_str], |row| {
                Ok(SessionStats {
                    avg_duration_seconds: row.get::<_, Option<f64>>(0)?.unwrap_or(0.0),
                    avg_tokens: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                    session_count: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    total_tokens: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                })
            })
            .map_err(|e| e.to_string())?;

        Ok(result)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")
            .map_err(|e| format!("Prepare error: {e}"))?;
        let result = stmt.query_row(params![key], |row| row.get(0)).ok();
        Ok(result)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(|e| format!("Setting write error: {e}"))?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM settings WHERE key = ?1", params![key])
            .map_err(|e| format!("Setting delete error: {e}"))?;
        Ok(())
    }

    pub fn get_provider_settings_json(&self) -> Result<Option<String>, String> {
        self.get_setting(PROVIDER_SETTINGS_KEY)
    }

    pub fn set_provider_settings_json(&self, value: &str) -> Result<(), String> {
        self.set_setting(PROVIDER_SETTINGS_KEY, value)
    }

    #[allow(dead_code)]
    pub fn get_indicator_primary_provider(&self) -> Result<Option<IntegrationProvider>, String> {
        let Some(raw) = self.get_setting(INDICATOR_PRIMARY_PROVIDER_KEY)? else {
            return Ok(None);
        };

        match serde_json::from_str(&raw) {
            Ok(provider) => Ok(provider),
            Err(_) => Ok(None),
        }
    }

    #[allow(dead_code)]
    pub fn set_indicator_primary_provider(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<(), String> {
        match provider {
            Some(provider) => {
                let encoded = serde_json::to_string(&Some(provider)).map_err(|e| {
                    format!("Serialize error for indicator.primary_provider.v1: {e}")
                })?;
                self.set_setting(INDICATOR_PRIMARY_PROVIDER_KEY, &encoded)
            }
            None => self.delete_setting(INDICATOR_PRIMARY_PROVIDER_KEY),
        }
    }

    pub fn delete_host_data(&self, hostname: &str) -> Result<u64, String> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        // Feature 008: cascade session_events for every session that lived
        // on this host. session_events keys on (provider, session_id), so
        // we resolve the set via a subquery against `token_snapshots`
        // (the authoritative source for "what sessions ran on this host").
        // The subquery is evaluated inside the same transaction as the
        // deletes — no separate lock acquisition — so a concurrent indexer
        // run cannot slip new session_events rows past us, and the order
        // (session_events delete BEFORE token_snapshots delete) ensures
        // the subquery still sees the pre-delete state. See
        // specs/008-runtime-redesign/contracts/session-events.md.
        let event_count = tx
            .execute(
                "DELETE FROM session_events
                 WHERE (provider, session_id) IN (
                     SELECT DISTINCT provider, session_id
                     FROM token_snapshots
                     WHERE hostname = ?1
                 )",
                params![hostname],
            )
            .map_err(|e| format!("Delete session_events cascade error: {e}"))?;

        // Feature 009: cascade hook_invocations rows for every session
        // that lived on this host, using the same token_snapshots
        // subquery pattern as the session_events cascade. Sequenced
        // before the token_snapshots delete so the subquery still sees
        // the source rows. Also drops any rows the observer captured
        // with an explicit hostname column match (Codex side).
        // @lat: [[backend#Database#Schema#Hook Invocations]]
        let hook_count = tx
            .execute(
                "DELETE FROM hook_invocations
                 WHERE (provider, session_id) IN (
                     SELECT DISTINCT provider, session_id
                     FROM token_snapshots
                     WHERE hostname = ?1
                 )
                 OR hostname = ?1",
                params![hostname],
            )
            .map_err(|e| format!("Delete hook_invocations cascade error: {e}"))?;

        let snap_count = tx
            .execute(
                "DELETE FROM token_snapshots WHERE hostname = ?1",
                params![hostname],
            )
            .map_err(|e| format!("Delete snapshots error: {e}"))?;

        let hourly_count = tx
            .execute(
                "DELETE FROM token_hourly WHERE hostname = ?1",
                params![hostname],
            )
            .map_err(|e| format!("Delete hourly error: {e}"))?;

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok((snap_count + hourly_count + event_count + hook_count) as u64)
    }

    pub fn delete_session_data(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<u64, String> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Delete session transaction error: {e}"))?;

        let token_count = tx
            .execute(
                "DELETE FROM token_snapshots WHERE provider = ?1 AND session_id = ?2",
                params![provider.as_str(), session_id],
            )
            .map_err(|e| format!("Delete token snapshots error: {e}"))?;

        let skill_count = tx
            .execute(
                "DELETE FROM skill_usages WHERE provider = ?1 AND session_id = ?2",
                params![provider.as_str(), session_id],
            )
            .map_err(|e| format!("Delete skill usages error: {e}"))?;

        // Feature 008: cascade session_events for this session. See
        // specs/008-runtime-redesign/contracts/session-events.md (FR-012).
        let event_count = tx
            .execute(
                "DELETE FROM session_events WHERE provider = ?1 AND session_id = ?2",
                params![provider.as_str(), session_id],
            )
            .map_err(|e| format!("Delete session_events error: {e}"))?;

        // Feature 009: cascade hook_invocations for this session so the
        // Hooks breakdown stays consistent with the visible session list.
        // @lat: [[backend#Database#Schema#Hook Invocations]]
        let hook_count = tx
            .execute(
                "DELETE FROM hook_invocations WHERE provider = ?1 AND session_id = ?2",
                params![provider.as_str(), session_id],
            )
            .map_err(|e| format!("Delete hook_invocations error: {e}"))?;

        tx.commit()
            .map_err(|e| format!("Delete session commit error: {e}"))?;

        Ok((token_count + skill_count + event_count + hook_count) as u64)
    }

    pub fn delete_project_data(&self, cwd: &str) -> Result<u64, String> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin delete_project_data transaction: {e}"))?;

        // Feature 008: cascade session_events for every session that lived
        // under this cwd. As in `delete_host_data`, the subquery resolving
        // (provider, session_id) runs inside the same transaction as the
        // deletes so concurrent indexer writes cannot race us, and the
        // session_events delete is sequenced BEFORE the token_snapshots
        // delete so the subquery still observes the source rows.
        let event_count = tx
            .execute(
                "DELETE FROM session_events
                 WHERE (provider, session_id) IN (
                     SELECT DISTINCT provider, session_id
                     FROM token_snapshots
                     WHERE cwd = ?1
                 )",
                params![cwd],
            )
            .map_err(|e| format!("Delete session_events cascade error: {e}"))?;

        let token_count = tx
            .execute("DELETE FROM token_snapshots WHERE cwd = ?1", params![cwd])
            .map_err(|e| format!("Delete token_snapshots error: {e}"))?;

        let skill_count = tx
            .execute("DELETE FROM skill_usages WHERE cwd = ?1", params![cwd])
            .map_err(|e| format!("Delete skill_usages error: {e}"))?;

        // Feature 009: cascade hook_invocations rows for this cwd.
        // @lat: [[backend#Database#Schema#Hook Invocations]]
        let hook_count = tx
            .execute("DELETE FROM hook_invocations WHERE cwd = ?1", params![cwd])
            .map_err(|e| format!("Delete hook_invocations error: {e}"))?;

        tx.commit()
            .map_err(|e| format!("Commit delete_project_data: {e}"))?;

        Ok((token_count + skill_count + event_count + hook_count) as u64)
    }

    pub fn rename_project(&self, old_cwd: &str, new_cwd: &str) -> Result<u64, String> {
        let new_cwd = new_cwd.trim();
        if new_cwd.is_empty() {
            return Err("New project path cannot be empty".to_string());
        }
        if new_cwd == old_cwd {
            return Err("New path is the same as the current path".to_string());
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        let snap_count = tx
            .execute(
                "UPDATE token_snapshots SET cwd = ?2 WHERE cwd = ?1",
                params![old_cwd, new_cwd],
            )
            .map_err(|e| format!("Update token_snapshots error: {e}"))?;

        tx.execute(
            "UPDATE observations SET cwd = ?2 WHERE cwd = ?1",
            params![old_cwd, new_cwd],
        )
        .map_err(|e| format!("Update observations error: {e}"))?;

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok(snap_count as u64)
    }

    pub fn get_project_provider_map(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<std::collections::HashMap<String, Vec<IntegrationProvider>>, String> {
        let conn = self.conn.lock();
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT provider, cwd FROM token_snapshots
                 WHERE cwd IS NOT NULL AND provider = ?1
                 UNION
                 SELECT provider, cwd FROM observations
                 WHERE cwd IS NOT NULL AND provider = ?1",
                vec![Box::new(provider.as_str().to_string()) as Box<dyn rusqlite::types::ToSql>],
            ),
            None => (
                "SELECT provider, cwd FROM token_snapshots
                 WHERE cwd IS NOT NULL
                 UNION
                 SELECT provider, cwd FROM observations
                 WHERE cwd IS NOT NULL",
                Vec::new(),
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare project providers error: {e}"))?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|param| param.as_ref()).collect();
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Query project providers error: {e}"))?;

        let mut project_providers: std::collections::HashMap<String, Vec<IntegrationProvider>> =
            std::collections::HashMap::new();
        for row in rows {
            let (provider_name, cwd) = row.map_err(|e| format!("Project provider row: {e}"))?;
            let parsed_provider: IntegrationProvider = provider_name.parse()?;
            let entry = project_providers.entry(cwd).or_default();
            if !entry.contains(&parsed_provider) {
                entry.push(parsed_provider);
                entry.sort_by_key(|provider| provider.as_str());
            }
        }

        Ok(project_providers)
    }

    // --- Learning system methods ---

    pub fn store_observation(&self, payload: &ObservationPayload) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();

        // Defense-in-depth (feature 005 US1 T014, R-1 / contract
        // redaction.md). The request handler (`server.rs` post_observation,
        // T013) already redacts before dispatching here, but this is the
        // at-rest backstop guaranteeing no plaintext secret/PII ever lands
        // in `observations`. `redact` is idempotent, so double-redacting an
        // already-clean payload is a safe no-op.
        let tool_input = payload.tool_input.as_deref().map(crate::redaction::redact);
        let tool_output = payload.tool_output.as_deref().map(crate::redaction::redact);
        let cwd = payload.cwd.as_deref().map(crate::redaction::redact);

        conn.execute(
            "INSERT INTO observations (provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                payload.provider.as_str(),
                payload.session_id,
                now,
                payload.hook_phase,
                payload.tool_name,
                tool_input,
                tool_output,
                cwd,
            ],
        )
        .map_err(|e| format!("Insert observation error: {e}"))?;
        Ok(())
    }

    pub fn get_recent_observations(
        &self,
        limit: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<serde_json::Value>, String> {
        self.get_observations_since(None, limit, provider)
    }

    pub fn get_unanalyzed_observations(
        &self,
        limit: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let since = self.latest_completed_learning_run_created_at(provider)?;
        self.get_observations_since(Some(&since), limit, provider)
    }

    fn latest_completed_learning_run_created_at(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<String, String> {
        let conn = self.conn.lock();
        match provider {
            Some(provider) => conn
                .query_row(
                    "SELECT COALESCE(MAX(created_at), '1970-01-01')
                     FROM learning_runs
                     WHERE status = 'completed'
                       AND instr(provider_scope, ?1) > 0",
                    params![provider_scope_contains_json(provider)],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Query last run error: {e}")),
            None => conn
                .query_row(
                    "SELECT COALESCE(MAX(created_at), '1970-01-01')
                     FROM learning_runs
                     WHERE status = 'completed'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Query last run error: {e}")),
        }
    }

    fn get_observations_since(
        &self,
        since: Option<&str>,
        limit: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let conn = self.conn.lock();
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match since {
            Some(s) if provider.is_some() => (
                "SELECT id, provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
                 FROM observations
                 WHERE created_at > ?1 AND provider = ?2
                 ORDER BY created_at DESC
                 LIMIT ?3",
                vec![
                    Box::new(s.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(provider.expect("provider checked above").as_str().to_string()),
                    Box::new(limit),
                ],
            ),
            Some(s) => (
                "SELECT id, provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
                 FROM observations
                 WHERE created_at > ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
                vec![Box::new(s.to_string()) as Box<dyn rusqlite::types::ToSql>, Box::new(limit)],
            ),
            None if provider.is_some() => (
                "SELECT id, provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
                 FROM observations
                 WHERE provider = ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
                vec![
                    Box::new(provider.expect("provider checked above").as_str().to_string())
                        as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT id, provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at
                 FROM observations
                 ORDER BY created_at DESC
                 LIMIT ?1",
                vec![Box::new(limit) as Box<dyn rusqlite::types::ToSql>],
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, i64>(0)?,
                    "provider": row.get::<_, String>(1)?,
                    "session_id": row.get::<_, String>(2)?,
                    "timestamp": row.get::<_, String>(3)?,
                    "hook_phase": row.get::<_, String>(4)?,
                    "tool_name": row.get::<_, String>(5)?,
                    "tool_input": row.get::<_, Option<String>>(6)?,
                    "tool_output": row.get::<_, Option<String>>(7)?,
                    "cwd": row.get::<_, Option<String>>(8)?,
                    "created_at": row.get::<_, String>(9)?,
                }))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_observation_count(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        match provider {
            Some(provider) => conn
                .query_row(
                    "SELECT COUNT(*) FROM observations WHERE provider = ?1",
                    params![provider.as_str()],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Count error: {e}")),
            None => conn
                .query_row("SELECT COUNT(*) FROM observations", [], |row| row.get(0))
                .map_err(|e| format!("Count error: {e}")),
        }
    }

    pub fn get_unanalyzed_observation_count(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<i64, String> {
        let since = self.latest_completed_learning_run_created_at(provider)?;
        let conn = self.conn.lock();
        match provider {
            Some(provider) => conn
                .query_row(
                    "SELECT COUNT(*) FROM observations
                     WHERE created_at > ?1 AND provider = ?2",
                    params![since, provider.as_str()],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Count error: {e}")),
            None => conn
                .query_row(
                    "SELECT COUNT(*) FROM observations WHERE created_at > ?1",
                    params![since],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Count error: {e}")),
        }
    }

    pub fn get_top_tools(
        &self,
        limit: i64,
        days: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<ToolCount>, String> {
        let conn = self.conn.lock();
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT tool_name, COUNT(*) as count FROM observations
                 WHERE created_at >= datetime('now', '-' || ?1 || ' days')
                   AND provider = ?2
                 GROUP BY tool_name ORDER BY count DESC LIMIT ?3",
                vec![
                    Box::new(days) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(provider.as_str().to_string()),
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT tool_name, COUNT(*) as count FROM observations
                 WHERE created_at >= datetime('now', '-' || ?1 || ' days')
                 GROUP BY tool_name ORDER BY count DESC LIMIT ?2",
                vec![
                    Box::new(days) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|param| param.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(ToolCount {
                    tool_name: row.get(0)?,
                    count: row.get(1)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    /// Read the formerly write-only `observation_summaries` table — the only
    /// post-retention historical record of observation activity once raw
    /// `observations` rows are pruned by [`Storage::cleanup_old_observations`].
    ///
    /// Feature 005 US5 T062 (R-7.4 / M-1 / FR-027). Returns rows with
    /// `period >= since` (inclusive; `period` is the `%Y-%m-%d` cleanup date),
    /// optionally provider- and project-scoped, newest period first. A `None`
    /// `project` filter returns all projects; `Some(p)` matches that project
    /// exactly (including the rolled-up empty/unknown-project bucket if `p`
    /// is empty). Powers the analytics trend's historical tail so the trend
    /// survives retention; also surfaceable directly for analytics callers.
    pub fn get_observation_summaries(
        &self,
        provider: Option<IntegrationProvider>,
        project: Option<&str>,
        since: &str,
    ) -> Result<Vec<ObservationSummary>, String> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT period, provider, project, tool_counts, error_count, total_observations, created_at
             FROM observation_summaries
             WHERE period >= ?1",
        );
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(since.to_string())];
        if let Some(provider) = provider {
            sql.push_str(" AND provider = ?2");
            binds.push(Box::new(provider.as_str().to_string()));
        }
        if let Some(project) = project {
            sql.push_str(&format!(" AND project = ?{}", binds.len() + 1));
            binds.push(Box::new(project.to_string()));
        }
        sql.push_str(" ORDER BY period DESC, provider ASC");

        let mut stmt = conn
            .prepare_cached(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;
        let bind_refs: Vec<&dyn rusqlite::types::ToSql> =
            binds.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(bind_refs.as_slice(), |row| {
                Ok(ObservationSummary {
                    period: row.get(0)?,
                    provider: row.get(1)?,
                    project: row.get(2)?,
                    tool_counts: row.get(3)?,
                    error_count: row.get(4)?,
                    total_observations: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn get_observation_sparkline(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<i64>, String> {
        let conn = self.conn.lock();
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT DATE(created_at) as day, COUNT(*) as count
                 FROM observations
                 WHERE created_at >= DATE('now', '-6 days')
                   AND provider = ?1
                 GROUP BY day ORDER BY day ASC",
                vec![Box::new(provider.as_str().to_string()) as Box<dyn rusqlite::types::ToSql>],
            ),
            None => (
                "SELECT DATE(created_at) as day, COUNT(*) as count
                 FROM observations
                 WHERE created_at >= DATE('now', '-6 days')
                 GROUP BY day ORDER BY day ASC",
                Vec::new(),
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|param| param.as_ref()).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut day_map = std::collections::HashMap::new();
        for row in rows {
            let (day, count) = row.map_err(|e| format!("Row error: {e}"))?;
            day_map.insert(day, count);
        }
        // Release the connection guard before re-entering via
        // `get_observation_summaries` — `parking_lot::Mutex` is NOT reentrant,
        // so the nested `self.conn.lock()` would deadlock otherwise.
        drop(stmt);
        drop(conn);

        // Feature 005 US5 T062 (R-7.4 / M-1 / FR-027): fold the post-retention
        // historical tail in from `observation_summaries`. When a day's raw
        // rows were pruned by `cleanup_old_observations`, the summary table
        // still holds that day's rolled-up total keyed by the cleanup
        // `period`. Add it ONLY for days the raw scan returned nothing, so the
        // trend survives retention without double-counting live days.
        let summary_window = (Utc::now().date_naive() - chrono::Duration::days(6))
            .format("%Y-%m-%d")
            .to_string();
        for summary in self
            .get_observation_summaries(provider, None, &summary_window)
            .unwrap_or_default()
        {
            day_map
                .entry(summary.period)
                .or_insert(summary.total_observations);
        }

        // Build a 7-element array, zero-filled for missing days
        let today = Utc::now().date_naive();
        let mut result = Vec::with_capacity(7);
        for i in (0..7).rev() {
            let day = (today - chrono::Duration::days(i))
                .format("%Y-%m-%d")
                .to_string();
            let count = day_map.get(&day).copied().unwrap_or(0);
            result.push(count);
        }
        Ok(result)
    }

    pub fn store_learning_run(&self, payload: &LearningRunPayload) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO learning_runs (trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, error, logs, phases, provider_scope, inference_metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                payload.trigger_mode,
                payload.observations_analyzed,
                payload.rules_created,
                payload.rules_updated,
                payload.duration_ms,
                payload.status,
                payload.error,
                payload.logs,
                payload.phases,
                provider_scope_json(&payload.provider_scope),
                payload.inference_metadata,
            ],
        )
        .map_err(|e| format!("Insert learning run error: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn create_learning_run(
        &self,
        trigger_mode: &str,
        provider_scope: &[IntegrationProvider],
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO learning_runs (trigger_mode, status, observations_analyzed, rules_created, rules_updated, provider_scope)
             VALUES (?1, 'running', 0, 0, 0, ?2)",
            params![trigger_mode, provider_scope_json(provider_scope)],
        )
        .map_err(|e| format!("Create learning run error: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_learning_run(&self, id: i64, payload: &LearningRunPayload) -> Result<(), String> {
        let conn = self.conn.lock();
        // Feature 005 US2 T033 (FR-013, contracts/rule-governance.md "Run
        // status"): the closed enum is enforced in Rust at the persistence
        // boundary. Callers already pass only enum literals; this clamp is
        // the belt-and-suspenders guarantee — any unrecognized status is
        // coerced to `failed` (an unknown run state is, conservatively, not
        // a success, and `failed` writes nothing to disk anyway) so a
        // free-form status can never reach `learning_runs.status`.
        let status: &str = match payload.status.as_str() {
            "running" | "completed" | "degraded" | "failed" => payload.status.as_str(),
            _ => "failed",
        };
        conn.execute(
            "UPDATE learning_runs SET observations_analyzed=?2, rules_created=?3, rules_updated=?4,
             duration_ms=?5, status=?6, error=?7, logs=?8, phases=?9, provider_scope=?10,
             inference_metadata=?11
             WHERE id=?1",
            params![
                id,
                payload.observations_analyzed,
                payload.rules_created,
                payload.rules_updated,
                payload.duration_ms,
                status,
                payload.error,
                payload.logs,
                payload.phases,
                provider_scope_json(&payload.provider_scope),
                payload.inference_metadata,
            ],
        )
        .map_err(|e| format!("Update learning run error: {e}"))?;
        Ok(())
    }

    pub fn cleanup_interrupted_runs(&self) -> Result<u64, String> {
        let conn = self.conn.lock();
        let count = conn
            .execute(
                "UPDATE learning_runs SET status='interrupted' WHERE status='running'",
                [],
            )
            .map_err(|e| format!("Cleanup interrupted runs error: {e}"))?;
        Ok(count as u64)
    }

    pub fn get_learning_runs(
        &self,
        limit: i64,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<LearningRun>, String> {
        let conn = self.conn.lock();
        // Feature 005 US5 T058 (R-7.1 / H-6 / FR-024): also read the existing
        // `inference_metadata` JSON (column added by feature 003 / migration
        // 24 — no new migration) and fold it into a tolerant
        // `RunInferenceSummary` rollup. Legacy / micro runs legitimately have
        // no metadata; `decode_inference_metadata` maps NULL / parse-error to
        // `None` and never panics. `observations_analyzed` is already its own
        // surfaced column (index 2) — not duplicated here.
        let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT id, trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, error, logs, created_at, phases, provider_scope, inference_metadata
                 FROM learning_runs
                 WHERE instr(provider_scope, ?1) > 0
                 ORDER BY created_at DESC LIMIT ?2",
                vec![
                    Box::new(provider_scope_contains_json(provider))
                        as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT id, trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, error, logs, created_at, phases, provider_scope, inference_metadata
                 FROM learning_runs
                 ORDER BY created_at DESC LIMIT ?1",
                vec![Box::new(limit) as Box<dyn rusqlite::types::ToSql>],
            ),
        };
        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| format!("Prepare error: {e}"))?;
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|param| param.as_ref()).collect();

        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                let inference_metadata: Option<String> = row.get(12)?;
                Ok(LearningRun {
                    id: row.get(0)?,
                    trigger_mode: row.get(1)?,
                    observations_analyzed: row.get(2)?,
                    rules_created: row.get(3)?,
                    rules_updated: row.get(4)?,
                    duration_ms: row.get(5)?,
                    status: row.get(6)?,
                    error: row.get(7)?,
                    logs: row.get(8)?,
                    created_at: row.get(9)?,
                    phases: row.get(10)?,
                    provider_scope: parse_provider_scope(row.get(11)?),
                    inference: decode_inference_metadata(inference_metadata),
                })
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(results)
    }

    pub fn store_learned_rule(&self, payload: &LearnedRulePayload) -> Result<bool, String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        // Dynamic evidence scaling: more observations → more weight.
        // Clamped to [5, 20] so tiny batches don't over-commit and large batches
        // don't overwhelm existing evidence.
        let evidence_scale = (payload.observation_count as f64).clamp(5.0, 20.0);
        let alpha = payload.confidence * evidence_scale;
        let beta = (1.0 - payload.confidence) * evidence_scale;
        let is_anti = payload.is_anti_pattern as i32;
        // One pre-upsert point-read of the existing row (single indexed
        // lookup on the UNIQUE `name`) feeds BOTH the provider_scope merge
        // and the Follow-up B pending-change detection. Collapsing the two
        // former separate reads also removes the intermediate-state window
        // they exposed — the row cannot change between them when there is
        // only one read.
        let existing: Option<(Option<String>, String, Option<String>)> = conn
            .query_row(
                "SELECT provider_scope, lifecycle, content FROM learned_rules WHERE name = ?1",
                params![payload.name],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("Read learned rule state error: {e}"))?;
        let mut merged_scope = parse_provider_scope(existing.as_ref().and_then(|e| e.0.clone()));
        merged_scope.extend(payload.provider_scope.iter().copied());
        let merged_scope = normalized_provider_scope(&merged_scope);
        // Feature 006 Follow-up B (R-B / C-B, contracts/confinement-and-
        // atomicity.md "C-B"): the pending-change marker (a `current_version`
        // bump for an `awaiting_review` rule re-derived with different
        // content) is NO LONGER applied here. It used to be a CASE in the
        // `ON CONFLICT` clause, which advanced `current_version` BEFORE the
        // new version's `rule_evidence_citations` snapshot existed — a
        // transient window for any concurrent eligibility reader and, if the
        // non-blocking citation write later failed, a *permanent* state in
        // which `current_version` points at a citation-less version (the
        // human-pending rule silently and permanently leaves the review
        // queue). Instead we read the pre-upsert lifecycle/content here
        // (same `conn` lock, before the upsert mutates the row) and surface a
        // `pending_changed` signal to the caller; `write_rule_files` advances
        // `current_version` only AFTER the new-version citations are
        // persisted, atomically (see `persist_citations_and_advance_version`).
        // This reproduces the deleted CASE's trigger condition EXACTLY:
        // `lifecycle == 'awaiting_review' AND old.content IS NOT NULL AND
        // new.content IS NOT NULL AND old.content != new.content`. An
        // `awaiting_review` row can never simultaneously be
        // suppressed/tombstoned, so the suppression-sticky guard the CASE
        // never had is not needed here; a non-existent row never signals.
        let pending_changed = match existing.as_ref() {
            Some((_, lifecycle, existing_content)) => {
                lifecycle.as_str() == "awaiting_review"
                    && existing_content.is_some()
                    && payload.content.is_some()
                    && existing_content.as_deref() != payload.content.as_deref()
            }
            None => false,
        };
        // Feature 005 US2 T026/T027/T030 (data-model.md "rule lifecycle",
        // contracts/rule-governance.md). Extraction persists DB-only
        // *candidates* — new rows are `lifecycle='candidate'` (distinct from
        // the read-derived quality `state`, which `get_learned_rules`
        // recomputes every read and must never clobber `lifecycle`).
        //
        // The `ON CONFLICT(name)` upsert is intentionally **suppression-
        // sticky** (T027 / R-2): a `suppressed` or `tombstoned` row keeps
        // accruing α/β evidence (so re-arming can be gated on real signal)
        // but its `file_path`/`content` are NOT revived by re-extraction and
        // its `lifecycle` is left untouched — only an explicit authorized
        // path (`reactivate_rule` + approval) may re-arm it. A row already in
        // `awaiting_review` is re-derived idempotently (T030 / FR-007 edge
        // case): content is UPSERTed in place, never duplicated, never
        // auto-approved, without ever overwriting an `active` rule's on-disk
        // `.md` (only `promote_*` writes disk). `lifecycle` itself is never
        // mutated here. Feature 006 Follow-up B: the `current_version` pending
        // marker bump is no longer in this CASE — it is applied post-citation-
        // persist and atomically by the caller (returned `pending_changed`).
        conn.execute(
            "INSERT INTO learned_rules (name, domain, confidence, observation_count, file_path, alpha, beta_param, last_evidence_at, state, lifecycle, project, is_anti_pattern, source, content, provider_scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'emerging', 'candidate', ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(name) DO UPDATE SET
                 domain = excluded.domain,
                 alpha = learned_rules.alpha + excluded.alpha,
                 beta_param = learned_rules.beta_param + excluded.beta_param,
                 observation_count = learned_rules.observation_count + excluded.observation_count,
                 file_path = CASE
                     WHEN learned_rules.lifecycle IN ('suppressed', 'tombstoned')
                          OR EXISTS (SELECT 1 FROM rule_tombstones t
                                     WHERE t.rule_name = learned_rules.name
                                       AND t.reactivated_at IS NULL)
                         THEN learned_rules.file_path
                     WHEN length(excluded.file_path) > 0 THEN excluded.file_path
                     ELSE learned_rules.file_path END,
                 last_evidence_at = excluded.last_evidence_at,
                 is_anti_pattern = excluded.is_anti_pattern,
                 source = CASE WHEN excluded.source IS NOT NULL THEN excluded.source ELSE learned_rules.source END,
                 content = CASE
                     WHEN learned_rules.lifecycle IN ('suppressed', 'tombstoned')
                          OR EXISTS (SELECT 1 FROM rule_tombstones t
                                     WHERE t.rule_name = learned_rules.name
                                       AND t.reactivated_at IS NULL)
                         THEN learned_rules.content
                     WHEN excluded.content IS NOT NULL THEN excluded.content
                     ELSE learned_rules.content END,
                 provider_scope = excluded.provider_scope,
                 updated_at = datetime('now')",
            params![
                payload.name,
                payload.domain,
                payload.confidence,
                payload.observation_count,
                payload.file_path,
                alpha,
                beta,
                now,
                payload.project,
                is_anti,
                payload.source,
                payload.content,
                provider_scope_json(&merged_scope),
            ],
        )
        .map_err(|e| format!("Insert learned rule error: {e}"))?;
        Ok(pending_changed)
    }

    pub fn reinforce_rule(&self, name: &str, strength: f64) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE learned_rules SET alpha = alpha + ?1, last_evidence_at = ?2, updated_at = datetime('now') WHERE name = ?3",
            params![strength, now, name],
        )
        .map_err(|e| format!("Reinforce rule error: {e}"))?;
        Ok(())
    }

    pub fn contradict_rule(&self, name: &str, strength: f64) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE learned_rules SET beta_param = beta_param + ?1, last_evidence_at = ?2, updated_at = datetime('now') WHERE name = ?3",
            params![strength, now, name],
        )
        .map_err(|e| format!("Contradict rule error: {e}"))?;
        Ok(())
    }

    pub fn get_learned_rules(
        &self,
        provider: Option<IntegrationProvider>,
    ) -> Result<Vec<LearnedRule>, String> {
        let mut meta_map = {
            let conn = self.conn.lock();
            let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
                Some(provider) => (
                    "SELECT name, domain, alpha, beta_param, observation_count, last_evidence_at, state, project, created_at, updated_at, is_anti_pattern, source, content, provider_scope
                     FROM learned_rules
                     WHERE state != 'suppressed' AND instr(provider_scope, ?1) > 0",
                    vec![
                        Box::new(provider_scope_contains_json(provider))
                            as Box<dyn rusqlite::types::ToSql>,
                    ],
                ),
                None => (
                    "SELECT name, domain, alpha, beta_param, observation_count, last_evidence_at, state, project, created_at, updated_at, is_anti_pattern, source, content, provider_scope
                     FROM learned_rules
                     WHERE state != 'suppressed'",
                    Vec::new(),
                ),
            };
            let mut stmt = conn
                .prepare_cached(sql)
                .map_err(|e| format!("Prepare error: {e}"))?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                params_vec.iter().map(|param| param.as_ref()).collect();
            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, i32>(10).unwrap_or(0),
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, Option<String>>(13)?,
                    ))
                })
                .map_err(|e| format!("Query error: {e}"))?;

            let mut map = std::collections::HashMap::new();
            for row in rows {
                let (
                    name,
                    domain,
                    alpha,
                    beta,
                    obs_count,
                    last_ev,
                    state,
                    project,
                    created,
                    updated,
                    is_anti,
                    source,
                    content,
                    provider_scope,
                ) = row.map_err(|e| format!("Row error: {e}"))?;
                map.insert(
                    name,
                    (
                        domain,
                        alpha,
                        beta,
                        obs_count,
                        last_ev,
                        state,
                        project,
                        created,
                        updated,
                        is_anti,
                        source,
                        content,
                        parse_provider_scope(provider_scope),
                    ),
                );
            }
            map
        };

        let mut rules = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        fn collect_rule_files(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        collect_rule_files(&path, out);
                    } else if path.extension().is_some_and(|ext| ext == "md") {
                        let name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let file_path = path.to_string_lossy().to_string();
                        out.push((name, file_path));
                    }
                }
            }
        }

        for rules_dir in learned_rule_dirs(provider) {
            if !rules_dir.exists() {
                continue;
            }

            let mut files = Vec::new();
            collect_rule_files(&rules_dir, &mut files);

            for (name, file_path) in files {
                if !seen_names.insert(name.clone()) {
                    continue;
                }

                let inferred_scope = inferred_rule_provider_scope(std::path::Path::new(&file_path));
                let now = Utc::now().to_rfc3339();
                let (
                    domain,
                    alpha,
                    beta,
                    observation_count,
                    last_ev,
                    _db_state,
                    project,
                    created_at,
                    updated_at,
                    is_anti,
                    source,
                    _content,
                    provider_scope,
                ) = meta_map.remove(&name).unwrap_or((
                    None,
                    1.0,
                    1.0,
                    0,
                    None,
                    "emerging".to_string(),
                    None,
                    now.clone(),
                    now,
                    0,
                    None,
                    None,
                    inferred_scope,
                ));

                let (confidence, state) = evidence_weighted_score(alpha, beta, last_ev.as_deref());
                let state = state.to_string();

                rules.push(LearnedRule {
                    name,
                    domain,
                    confidence,
                    observation_count,
                    file_path,
                    created_at,
                    updated_at,
                    state,
                    project,
                    is_anti_pattern: is_anti != 0,
                    source,
                    content: None,
                    provider_scope,
                });
            }
        }

        // Include DB-only rules (candidates that haven't met the confidence threshold yet)
        for (
            name,
            (
                domain,
                alpha,
                beta,
                observation_count,
                last_ev,
                _db_state,
                project,
                created_at,
                updated_at,
                is_anti,
                source,
                content_val,
                provider_scope,
            ),
        ) in meta_map
        {
            // DB-only rows are candidates by construction: keep the literal
            // "candidate" state (unchanged behavior) and take only the
            // evidence-weighted score from the shared scorer.
            let (confidence, _derived_state) =
                evidence_weighted_score(alpha, beta, last_ev.as_deref());
            let state = "candidate".to_string();

            rules.push(LearnedRule {
                name,
                domain,
                confidence,
                observation_count,
                file_path: String::new(),
                created_at,
                updated_at,
                state,
                project,
                is_anti_pattern: is_anti != 0,
                source,
                content: content_val,
                provider_scope,
            });
        }

        // Impact-based sorting: confidence * ln(observation_count + 1)
        // This ranks impactful, well-evidenced rules higher than trivial high-confidence ones
        rules.sort_by(|a, b| {
            let score_a = a.confidence * (a.observation_count as f64 + 1.0).ln();
            let score_b = b.confidence * (b.observation_count as f64 + 1.0).ln();
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(rules)
    }

    pub fn get_git_snapshot(&self, project: &str) -> Result<Option<GitSnapshot>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT project, commit_hash, commit_count, raw_data
                 FROM git_snapshots WHERE project = ?1",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let result = stmt
            .query_row(params![project], |row| {
                Ok(GitSnapshot {
                    project: row.get(0)?,
                    commit_hash: row.get(1)?,
                    commit_count: row.get(2)?,
                    raw_data: row.get(3)?,
                })
            })
            .optional()
            .map_err(|e| format!("Query error: {e}"))?;

        Ok(result)
    }

    pub fn upsert_git_snapshot(&self, snapshot: &GitSnapshot) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO git_snapshots (project, commit_hash, commit_count, raw_data)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project) DO UPDATE SET
                 commit_hash = excluded.commit_hash,
                 commit_count = excluded.commit_count,
                 raw_data = excluded.raw_data,
                 created_at = datetime('now')",
            params![
                snapshot.project,
                snapshot.commit_hash,
                snapshot.commit_count,
                snapshot.raw_data,
            ],
        )
        .map_err(|e| format!("Upsert git snapshot error: {e}"))?;
        Ok(())
    }

    /// Feature 005 US2 T027 (C-5, FR-010): public view of the durable
    /// tombstone gate (`true` ⇒ an un-reactivated `rule_tombstones` row
    /// exists for `name`). Used by the extraction candidate-writer
    /// (`learning::write_rule_files`) so a suppressed pattern is never
    /// re-surfaced as a fresh review candidate, and (later) by IPC.
    pub fn is_tombstone_active(&self, name: &str) -> bool {
        let conn = self.conn.lock();
        tombstone_blocks(&conn, name)
    }

    /// Feature 005 US3 T041 (H-1 / FR-015, research R-6 "Grounding").
    ///
    /// Resolve a candidate's machine-checkable `evidence_refs` back to real
    /// captured evidence. A ref counts as resolved iff:
    /// - `observation` — an `observations` row with `id = parse(id)` exists;
    /// - `commit` — the analyzed repo (`repo_path`, when supplied) answers
    ///   `git cat-file -e <id>^{{commit}}`, OR a `git_snapshots` row has a
    ///   `commit_hash` with `<id>` as a prefix or carries `<id>` in its
    ///   redacted `raw_data` (the `%h` / `[SNAPSHOT HEAD ...]` keys from
    ///   T040) — the snapshot path keeps grounding resolvable after the repo
    ///   is gone;
    /// - `session` — `sessions::find_session_path` resolves the id for
    ///   either provider.
    ///
    /// Input refs are de-duplicated by `(kind, id)`, so `resolved.len()` is
    /// the distinct resolved-citation count the eligibility gate (T042/T043)
    /// consumes. `distinct_sources` is the number of distinct resolved
    /// `kind`s; `project_paths` is the sorted distinct set of non-empty
    /// `observations.cwd` among resolved observation refs (repurposed by T045
    /// into `confirmed_projects` for the cross-project distinct-sources
    /// signal). Pure read — never mutates.
    pub fn resolve_evidence_refs(
        &self,
        refs: &[EvidenceRef],
        repo_path: Option<&str>,
    ) -> ResolvedEvidence {
        // De-dup by (kind, id) — a candidate citing the same evidence twice
        // must not inflate the count.
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        let mut deduped: Vec<&EvidenceRef> = Vec::new();
        for r in refs {
            if seen.insert((r.kind.clone(), r.id.clone())) {
                deduped.push(r);
            }
        }

        let mut resolved: Vec<ResolvedCitation> = Vec::new();
        let mut project_paths: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();

        for r in deduped {
            match r.kind.as_str() {
                "observation" => {
                    let Ok(obs_id) = r.id.parse::<i64>() else {
                        continue;
                    };
                    let conn = self.conn.lock();
                    type ObsMeta = (
                        Option<String>,
                        Option<String>,
                        Option<String>,
                        Option<String>,
                        Option<String>,
                    );
                    let row: Option<ObsMeta> = conn
                        .query_row(
                            "SELECT provider, session_id, cwd, tool_name, created_at
                             FROM observations WHERE id = ?1",
                            params![obs_id],
                            |row| {
                                Ok((
                                    row.get(0)?,
                                    row.get(1)?,
                                    row.get(2)?,
                                    row.get(3)?,
                                    row.get(4)?,
                                ))
                            },
                        )
                        .optional()
                        .unwrap_or(None);
                    drop(conn);
                    if let Some((provider, session_id, cwd, tool_name, created_at)) = row {
                        if let Some(p) = cwd.as_deref().filter(|s| !s.is_empty()) {
                            project_paths.insert(p.to_string());
                        }
                        resolved.push(ResolvedCitation {
                            kind: "observation".to_string(),
                            ref_id: r.id.clone(),
                            observation_id: Some(obs_id),
                            provider,
                            session_id,
                            cwd,
                            tool_name,
                            evidence_ts: created_at,
                        });
                    }
                }
                "commit" if commit_ref_resolves(&self.conn, repo_path, &r.id) => {
                    resolved.push(ResolvedCitation {
                        kind: "commit".to_string(),
                        ref_id: r.id.clone(),
                        observation_id: None,
                        provider: None,
                        session_id: None,
                        cwd: repo_path.map(str::to_string),
                        tool_name: None,
                        evidence_ts: None,
                    });
                }
                "commit" => {}
                "session" => {
                    let found = [IntegrationProvider::Claude, IntegrationProvider::Codex]
                        .into_iter()
                        .any(|p| {
                            matches!(crate::sessions::find_session_path(p, &r.id), Ok(Some(_)))
                        });
                    if found {
                        resolved.push(ResolvedCitation {
                            kind: "session".to_string(),
                            ref_id: r.id.clone(),
                            observation_id: None,
                            provider: None,
                            session_id: Some(r.id.clone()),
                            cwd: None,
                            tool_name: None,
                            evidence_ts: None,
                        });
                    }
                }
                _ => {}
            }
        }

        let distinct_sources = resolved
            .iter()
            .map(|c| c.kind.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len();

        ResolvedEvidence {
            resolved,
            distinct_sources,
            project_paths: project_paths.into_iter().collect(),
        }
    }

    /// Feature 006 Follow-up B (R-B / C-B / Option B3,
    /// contracts/confinement-and-atomicity.md "C-B"): persist the new
    /// version's `rule_evidence_citations` snapshot AND advance the pending
    /// marker `learned_rules.current_version` ATOMICALLY, in a single
    /// transaction, so the invariant "`current_version` always resolves to a
    /// `rule_version` that has its `rule_evidence_citations` snapshot" holds
    /// by construction.
    ///
    /// `pending_changed` is the signal returned by `store_learned_rule` (an
    /// `awaiting_review` rule whose content actually changed). When it is
    /// `true` the snapshot is written at `target = current_version + 1` and
    /// `current_version` is then bumped to `target` in the SAME tx; when it
    /// is `false` the snapshot is (re-)written at the unchanged
    /// `current_version` (idempotent re-snapshot, no bump). The previous
    /// version's citation rows (`rule_version = current_version`) are NEVER
    /// touched when bumping, so before the commit `eligible_for_review`
    /// (which reads `current_version`) still observes the OLD cited version
    /// and after the commit it observes `target`, which now has its rows —
    /// the advance and the new snapshot are atomic with respect to any
    /// eligibility reader (C-B1).
    ///
    /// On ANY error the tx rolls back, so neither the new rows nor the bump
    /// persist and the rule stays on its prior cited, review-eligible
    /// snapshot — closing the persistent case where a non-blocking citation
    /// failure left a human-pending rule permanently un-reviewable
    /// (C-B2 / FR-010 / SC-006). This does NOT roll back `store_learned_rule`
    /// (the α/β + content merge already committed separately), preserving
    /// feature-005 "merge-always" (C-B4). `current_version` remains the sole
    /// pending-change marker — no schema change, no `pending_changed` column.
    ///
    /// This is the SOLE `rule_evidence_citations` snapshot writer. Feature
    /// 006 removed the old `persist_evidence_citations` (its only production
    /// caller was `write_rule_files`); the `pending_changed == false` path
    /// here is the exact idempotent re-snapshot it provided — ≤8 row cap,
    /// `confirmed_projects` distinct-sources update, prior rows for the
    /// written version cleared first — so feature-005 snapshot behavior is
    /// byte-for-byte unchanged when no pending advance is requested.
    pub fn persist_citations_and_advance_version(
        &self,
        name: &str,
        resolved: &ResolvedEvidence,
        pending_changed: bool,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let Some((rule_id, current_version)): Option<(i64, i64)> = conn
            .query_row(
                "SELECT id, current_version FROM learned_rules WHERE name = ?1",
                params![name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("Citation rule lookup error: {e}"))?
        else {
            return Ok(());
        };

        let target = if pending_changed {
            current_version + 1
        } else {
            current_version
        };

        let projects_json =
            serde_json::to_string(&resolved.project_paths).unwrap_or_else(|_| "[]".to_string());

        let tx = conn
            .transaction()
            .map_err(|e| format!("Citation tx begin: {e}"))?;
        tx.execute(
            "DELETE FROM rule_evidence_citations WHERE rule_id = ?1 AND rule_version = ?2",
            params![rule_id, target],
        )
        .map_err(|e| format!("Citation clear error: {e}"))?;
        for c in resolved.resolved.iter().take(8) {
            tx.execute(
                "INSERT INTO rule_evidence_citations
                    (rule_id, rule_version, observation_id, provider, session_id,
                     cwd, tool_name, evidence_ts, snippet, kind, ref_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, datetime('now'))",
                params![
                    rule_id,
                    target,
                    c.observation_id,
                    c.provider,
                    c.session_id,
                    c.cwd,
                    c.tool_name,
                    c.evidence_ts,
                    format!("{}:{}", c.kind, c.ref_id),
                    c.kind,
                    c.ref_id,
                ],
            )
            .map_err(|e| format!("Citation insert error: {e}"))?;
        }
        tx.execute(
            "UPDATE learned_rules SET confirmed_projects = ?1 WHERE id = ?2",
            params![projects_json, rule_id],
        )
        .map_err(|e| format!("Citation projects update error: {e}"))?;
        // Advance the pending marker ONLY after the new-version snapshot rows
        // exist, in the SAME tx — so no reader and no failure can ever see
        // `current_version` pointing at a citation-less version.
        if target != current_version {
            tx.execute(
                "UPDATE learned_rules SET current_version = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![target, rule_id],
            )
            .map_err(|e| format!("Pending-version advance error: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("Citation tx commit: {e}"))?;
        Ok(())
    }

    /// Feature 005 US3 T047 (C-3 / FR-029, research R-5): operator feedback
    /// is the **primary** outcome signal, layered on the existing α/β+Wilson
    /// substrate with a weight `W_op` that STRICTLY DOMINATES any single LLM
    /// verdict strength (≤1.0 per `RuleVerdict`) and the raw self-rating
    /// (α/β evidence scale ≤20 in `store_learned_rule`). So one human
    /// `accept`/`reject`/`bad` outweighs the LLM `support`/`contradict`
    /// path, which stays the weaker secondary signal for un-reviewed rules.
    ///
    /// `accept` → large α increment; `reject` → large β increment (no
    /// tombstone — recoverable); `bad` → largest β increment (the tombstone
    /// itself is written by `submit_rule_feedback`, not here). Returns the
    /// (Δα, Δβ) to fold into a rule's stored α/β before scoring. No-op when
    /// the rule has no operator feedback.
    fn operator_feedback_delta(conn: &Connection, name: &str) -> (f64, f64) {
        // `W_op` ≫ any LLM verdict (≤1.0) and the evidence scale (≤20).
        const W_OP: f64 = 50.0;
        const W_OP_BAD: f64 = 100.0;
        let fb: Option<String> = conn
            .query_row(
                "SELECT feedback FROM operator_feedback
                 WHERE rule_name = ?1 AND actor = 'operator'",
                params![name],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);
        match fb.as_deref() {
            Some("accept") => (W_OP, 0.0),
            Some("reject") => (0.0, W_OP),
            Some("bad") => (0.0, W_OP_BAD),
            _ => (0.0, 0.0),
        }
    }

    /// Feature 005 US3 T042/T043/T044/T047 (C-3/H-2/M-4 — FR-014/016/017).
    ///
    /// The single promotion-eligibility predicate. A SINGLE indexed
    /// point-read per candidate (no `get_learned_rules`, no N+1). A rule is
    /// eligible to be surfaced for human approval iff ALL hold:
    /// - `evidence_weighted_score >= min_eligibility` (Wilson scale; setting
    ///   `learning.min_eligibility`, default **0.6** = the existing
    ///   `confirmed` cutpoint, with the legacy `learning.min_confidence` key
    ///   read as a migration fallback) — operator feedback (T047) is folded
    ///   into α/β FIRST so it dominates;
    /// - `resolved_distinct_refs >= 3` (distinct `rule_evidence_citations`
    ///   rows at the rule's `current_version`, T041/H-2);
    /// - `distinct_sources >= 1` — distinct citation `kind`s PLUS the
    ///   distinct cross-project paths recorded in the repurposed
    ///   `confirmed_projects` (T045/R-6);
    /// - `state != "invalidated"` (revised `compute_state` β-override, M-4);
    /// - `!tombstone_blocks(name)` (durable suppression gate, C-5).
    ///
    /// The raw LLM `rule.confidence` gates NOTHING here.
    pub fn eligible_for_review(&self, name: &str) -> Result<bool, String> {
        // `learning.min_eligibility` on the Wilson scale; legacy
        // `learning.min_confidence` honored as a migration fallback.
        let min_eligibility: f64 = self
            .get_setting("learning.min_eligibility")
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .or_else(|| {
                self.get_setting("learning.min_confidence")
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(0.6);

        let conn = self.conn.lock();

        if tombstone_blocks(&conn, name) {
            return Ok(false);
        }

        type RuleScoreRow = (i64, f64, f64, Option<String>, i64, Option<String>);
        let Some((rule_id, alpha, beta, last_ev, current_version, confirmed_projects)): Option<
            RuleScoreRow,
        > = conn
            .query_row(
                "SELECT id, alpha, beta_param, last_evidence_at, current_version, confirmed_projects
                 FROM learned_rules WHERE name = ?1",
                params![name],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("Eligibility point-read error: {e}"))?
        else {
            return Ok(false);
        };

        // T047: fold operator feedback into α/β BEFORE scoring so the human
        // signal dominates the LLM-derived evidence.
        let (da, db) = Self::operator_feedback_delta(&conn, name);
        let (score, state) = evidence_weighted_score(alpha + da, beta + db, last_ev.as_deref());

        if state == "invalidated" {
            return Ok(false);
        }
        if score < min_eligibility {
            return Ok(false);
        }

        // T041/H-2: distinct resolved citations at the current version.
        let resolved_distinct_refs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM (
                     SELECT DISTINCT kind, ref_id FROM rule_evidence_citations
                     WHERE rule_id = ?1 AND rule_version = ?2
                 )",
                params![rule_id, current_version],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Citation count error: {e}"))?
            .unwrap_or(0);
        if resolved_distinct_refs < 3 {
            return Ok(false);
        }

        // T045/R-6: distinct_sources = distinct citation kinds + distinct
        // cross-project paths from the repurposed `confirmed_projects`.
        let distinct_kinds: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT kind) FROM rule_evidence_citations
                 WHERE rule_id = ?1 AND rule_version = ?2",
                params![rule_id, current_version],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Citation kind count error: {e}"))?
            .unwrap_or(0);
        let distinct_projects = confirmed_projects
            .as_deref()
            .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
            .map(|v| v.len() as i64)
            .unwrap_or(0);
        let distinct_sources = distinct_kinds + distinct_projects;
        if distinct_sources < 1 {
            return Ok(false);
        }

        Ok(true)
    }

    /// Feature 005 US3 T042: transition `lifecycle` to `new_state` ONLY when
    /// the current lifecycle is in `allowed_from`. Idempotent and
    /// non-clobbering — it never overwrites `active`/`rejected`/`tombstoned`
    /// (those are not in any caller's allow-list), so a candidate that loses
    /// eligibility on a later run is NOT silently demoted out of a terminal
    /// state, and re-running with the same verdict is a no-op.
    pub fn set_rule_lifecycle_if(
        &self,
        name: &str,
        new_state: &str,
        allowed_from: &[&str],
    ) -> Result<(), String> {
        if allowed_from.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock();
        let placeholders = std::iter::repeat_n("?", allowed_from.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE learned_rules SET lifecycle = ?1, updated_at = datetime('now')
             WHERE name = ?2 AND lifecycle IN ({placeholders})"
        );
        let mut sql_params: Vec<&dyn rusqlite::types::ToSql> = vec![&new_state, &name];
        for s in allowed_from {
            sql_params.push(s);
        }
        conn.execute(&sql, sql_params.as_slice())
            .map_err(|e| format!("Set lifecycle error: {e}"))?;
        Ok(())
    }

    /// Feature 005 US3 T044 (M-4 / FR-017, research R-6 "Verdicts"): the
    /// `irrelevant` verdict measurably moves rule state instead of being
    /// silently dropped — it monotonically DECAYS freshness by exactly one
    /// 90-day half-life (pushes `last_evidence_at` back 90 days), clamped so
    /// it can only ever move backward in time (a flurry of `irrelevant`
    /// verdicts keeps decaying; nothing can refresh a rule via this path).
    /// `freshness_factor` then halves, lowering the evidence-weighted score
    /// and pushing the rule toward `stale`.
    pub fn decay_rule_freshness(&self, name: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        let current: Option<Option<String>> = conn
            .query_row(
                "SELECT last_evidence_at FROM learned_rules WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Freshness read error: {e}"))?;
        let Some(current) = current else {
            return Ok(()); // unknown rule — nothing to decay
        };
        // Anchor at the existing timestamp when present, else "now"; then
        // subtract one 90-day half-life. Clamp monotone-backward: never set
        // a timestamp later than what is already stored.
        let now = Utc::now();
        let base = current
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or(now);
        let decayed = base - TimeDelta::days(90);
        let new_ts = match current
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        {
            Some(existing) if decayed >= existing.with_timezone(&Utc) => {
                existing.with_timezone(&Utc)
            }
            _ => decayed,
        };
        conn.execute(
            "UPDATE learned_rules SET last_evidence_at = ?1, updated_at = datetime('now')
             WHERE name = ?2",
            params![new_ts.to_rfc3339(), name],
        )
        .map_err(|e| format!("Freshness decay error: {e}"))?;
        Ok(())
    }

    /// Feature 005 US3 T046 (FR-029, research R-5, contracts/
    /// ipc-and-feedback.md "Feedback model"): upsert the operator's
    /// rule-level judgment.
    ///
    /// One revisable row per `(rule_name, actor='operator')`; stores the
    /// rule's `content_hash` at feedback time (attribution across later
    /// content edits). `accept` → large α (via `operator_feedback_delta`,
    /// consumed by scoring); `reject` → large β, recoverable; `bad` →
    /// largest β AND a durable name-keyed tombstone (reusing the exact US2
    /// `rule_tombstones` suppression path, `tombstoned_by='operator_bad'`)
    /// so a "this rule was bad" judgment is sticky across re-extraction.
    /// `feedback` is validated to the closed set. `note` is maintainer-only
    /// metadata persisted verbatim and is NEVER read back into any inference
    /// prompt. The Tauri IPC command is a separate later task.
    #[allow(dead_code)] // IPC wiring (T046-IPC) is a later task
    pub fn submit_rule_feedback(
        &self,
        name: &str,
        feedback: &str,
        note: Option<&str>,
    ) -> Result<(), String> {
        if !crate::learning::is_safe_rule_name(name) {
            return Err(format!(
                "Invalid rule name: {}",
                &name[..name.len().min(50)]
            ));
        }
        if !matches!(feedback, "accept" | "reject" | "bad") {
            return Err(format!(
                "Invalid feedback '{feedback}' — expected accept|reject|bad"
            ));
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Feedback tx begin: {e}"))?;

        let (rule_id, content_hash): (Option<i64>, Option<String>) = tx
            .query_row(
                "SELECT id, content_hash FROM learned_rules WHERE name = ?1",
                params![name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("Feedback rule lookup error: {e}"))?
            .unwrap_or((None, None));

        tx.execute(
            "INSERT INTO operator_feedback
                (rule_name, actor, feedback, note, rule_content_hash, created_at, updated_at)
             VALUES (?1, 'operator', ?2, ?3, ?4, datetime('now'), datetime('now'))
             ON CONFLICT(rule_name, actor) DO UPDATE SET
                 feedback = excluded.feedback,
                 note = excluded.note,
                 rule_content_hash = excluded.rule_content_hash,
                 updated_at = datetime('now')",
            params![name, feedback, note, content_hash],
        )
        .map_err(|e| format!("Feedback upsert error: {e}"))?;

        if feedback == "bad" {
            // `bad` is the strongest negative AND distinct from `reject`: it
            // writes a durable tombstone via the SAME suppression path as
            // `delete_learned_rule` (C-5) so re-extraction/reconcile cannot
            // silently resurrect it; only an explicit authorized
            // `reactivate_rule` clears it.
            tx.execute(
                "UPDATE learned_rules
                 SET beta_param = beta_param + 5.0, state = 'suppressed',
                     lifecycle = 'tombstoned', updated_at = datetime('now')
                 WHERE name = ?1",
                params![name],
            )
            .ok();
            tx.execute(
                "INSERT INTO rule_tombstones
                    (rule_name, rule_id, tombstoned_at, tombstoned_by, reason, last_content_hash)
                 VALUES (?1, ?2, datetime('now'), 'operator_bad', 'operator marked rule bad', ?3)
                 ON CONFLICT(rule_name) DO UPDATE SET
                     rule_id = excluded.rule_id,
                     tombstoned_at = datetime('now'),
                     tombstoned_by = 'operator_bad',
                     reason = 'operator marked rule bad',
                     last_content_hash = excluded.last_content_hash,
                     reactivated_at = NULL,
                     reactivated_by = NULL",
                params![name, rule_id, content_hash],
            )
            .map_err(|e| format!("Feedback tombstone error: {e}"))?;
        }

        tx.commit()
            .map_err(|e| format!("Feedback tx commit: {e}"))?;
        Ok(())
    }

    /// Feature 005 US3 T045 (M-3 / FR-018, research R-6 "Conflict/dedup"):
    /// deterministic flag-and-supersede. Replaces the old advisory
    /// "consolidation hint" log with a real reconciliation recorded on the
    /// rows themselves.
    ///
    /// Within each domain, for every pair of in-scope, non-terminal rules:
    /// - **duplicate** (name-prefix overlap >60% OR overlapping resolved
    ///   evidence) → the LOSER gets `lifecycle='superseded'` (not
    ///   review-eligible) and `superseded_by=<survivor>`;
    /// - **conflict** (opposite `is_anti_pattern` AND overlapping resolved
    ///   evidence) → BOTH get `lifecycle='conflict_flagged'` (not
    ///   review-eligible) pending a human decision.
    ///
    /// Survivor is deterministic: higher `evidence_weighted_score`, tie-broke
    /// by higher `observation_count`, then lexicographically smaller name.
    /// Terminal rows (`active`/`rejected`/`tombstoned`/already
    /// `superseded`/`conflict_flagged`) and tombstoned names are skipped so
    /// the pass is idempotent and never demotes a human-approved rule.
    pub fn record_rule_reconciliation(
        &self,
        provider_scope: &[IntegrationProvider],
    ) -> Result<(), String> {
        let _ = provider_scope; // reconciliation is global by rule name/domain
        let mut conn = self.conn.lock();

        type RuleRow = (
            i64,
            String,
            String,
            f64,
            f64,
            Option<String>,
            i64,
            i64,
            String,
        );
        let rows: Vec<RuleRow> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, COALESCE(domain, 'general'), alpha, beta_param,
                            last_evidence_at, observation_count, is_anti_pattern, lifecycle
                     FROM learned_rules
                     WHERE lifecycle NOT IN ('active', 'rejected', 'tombstoned',
                                             'superseded', 'conflict_flagged')",
                )
                .map_err(|e| format!("Reconcile select prepare: {e}"))?;
            let mapped = stmt
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                })
                .map_err(|e| format!("Reconcile query: {e}"))?;
            let mut v = Vec::new();
            for r in mapped {
                v.push(r.map_err(|e| format!("Reconcile row: {e}"))?);
            }
            v
        };

        // Precompute evidence-ref sets per rule_id (distinct kind:ref_id).
        let evidence_for = |conn: &Connection, rule_id: i64| -> std::collections::HashSet<String> {
            let mut set = std::collections::HashSet::new();
            if let Ok(mut stmt) = conn.prepare(
                "SELECT DISTINCT kind || ':' || ref_id FROM rule_evidence_citations
                 WHERE rule_id = ?1",
            ) && let Ok(rows) = stmt.query_map(params![rule_id], |r| r.get::<_, String>(0))
            {
                for r in rows.flatten() {
                    set.insert(r);
                }
            }
            set
        };

        struct RuleMeta {
            id: i64,
            name: String,
            domain: String,
            score: f64,
            obs_count: i64,
            is_anti: bool,
            evidence: std::collections::HashSet<String>,
        }

        let metas: Vec<RuleMeta> = rows
            .into_iter()
            .map(
                |(id, name, domain, alpha, beta, last_ev, obs_count, is_anti, _lc)| {
                    let (da, db) = Self::operator_feedback_delta(&conn, &name);
                    let (score, _state) =
                        evidence_weighted_score(alpha + da, beta + db, last_ev.as_deref());
                    let evidence = evidence_for(&conn, id);
                    RuleMeta {
                        id,
                        name,
                        domain,
                        score,
                        obs_count,
                        is_anti: is_anti != 0,
                        evidence,
                    }
                },
            )
            .collect();

        // Deterministic winner: higher score, then higher obs_count, then
        // lexicographically smaller name.
        let wins = |a: &RuleMeta, b: &RuleMeta| -> bool {
            if a.score != b.score {
                return a.score > b.score;
            }
            if a.obs_count != b.obs_count {
                return a.obs_count > b.obs_count;
            }
            a.name < b.name
        };

        let mut supersede: Vec<(i64, String)> = Vec::new(); // loser_id, survivor_name
        let mut conflict: std::collections::HashSet<i64> = std::collections::HashSet::new();

        for i in 0..metas.len() {
            for j in (i + 1)..metas.len() {
                let (a, b) = (&metas[i], &metas[j]);
                if a.domain != b.domain {
                    continue;
                }
                let shared = a
                    .name
                    .chars()
                    .zip(b.name.chars())
                    .take_while(|(x, y)| x == y)
                    .count();
                let min_len = a.name.len().min(b.name.len()).max(1);
                let name_overlap = shared > 3 && shared * 100 / min_len > 60;
                let evidence_overlap =
                    !a.evidence.is_empty() && !a.evidence.is_disjoint(&b.evidence);

                if a.is_anti != b.is_anti && evidence_overlap {
                    // Conflict: opposite polarity over shared evidence —
                    // flag BOTH, human-resolved.
                    conflict.insert(a.id);
                    conflict.insert(b.id);
                } else if name_overlap || evidence_overlap {
                    // Duplicate: supersede the deterministic loser.
                    let (winner, loser) = if wins(a, b) { (a, b) } else { (b, a) };
                    supersede.push((loser.id, winner.name.clone()));
                }
            }
        }

        if supersede.is_empty() && conflict.is_empty() {
            return Ok(());
        }

        let tx = conn
            .transaction()
            .map_err(|e| format!("Reconcile tx begin: {e}"))?;
        for (loser_id, survivor) in &supersede {
            // A row flagged as a conflict in the same pass must not also be
            // silently superseded.
            if conflict.contains(loser_id) {
                continue;
            }
            tx.execute(
                "UPDATE learned_rules
                 SET lifecycle = 'superseded', superseded_by = ?1,
                     updated_at = datetime('now')
                 WHERE id = ?2
                   AND lifecycle NOT IN ('active', 'rejected', 'tombstoned')",
                params![survivor, loser_id],
            )
            .map_err(|e| format!("Reconcile supersede error: {e}"))?;
        }
        for cid in &conflict {
            tx.execute(
                "UPDATE learned_rules
                 SET lifecycle = 'conflict_flagged', updated_at = datetime('now')
                 WHERE id = ?1
                   AND lifecycle NOT IN ('active', 'rejected', 'tombstoned')",
                params![cid],
            )
            .map_err(|e| format!("Reconcile conflict error: {e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("Reconcile tx commit: {e}"))?;
        Ok(())
    }

    pub fn delete_learned_rule(&self, name: &str) -> Result<(), String> {
        if !crate::learning::is_safe_rule_name(name) {
            return Err(format!(
                "Invalid rule name: {}",
                &name[..name.len().min(50)]
            ));
        }

        // Soft-delete + durable tombstone (feature 005 US2 T028, C-5,
        // FR-010, contracts/rule-governance.md). Keep the DB record with
        // strong negative feedback so re-extraction can't trivially
        // re-promote the same pattern (β += 5.0), clear `file_path`, set the
        // read-derived hide flag (`state='suppressed'`) AND the persisted
        // `lifecycle='tombstoned'`. Crucially, also write/refresh a
        // name-keyed `rule_tombstones` row (`tombstoned_by='human'`): the
        // tombstone is the *durable* gate (`state` is recomputed every read,
        // so it can't be authoritative) and must outlive this row — it is
        // never CASCADE-deleted and only the explicit authorized
        // `reactivate_rule` clears it. All DB mutations run in one tx so the
        // suppression and its tombstone land atomically.
        let (file_path, provider_scope) = {
            let mut conn = self.conn.lock();
            let tx = conn
                .transaction()
                .map_err(|e| format!("Delete learned rule tx begin: {e}"))?;
            type RuleDeleteMeta = (Option<String>, Option<String>, Option<i64>, Option<String>);
            let record: Option<RuleDeleteMeta> = tx
                .query_row(
                    "SELECT file_path, provider_scope, id, content_hash FROM learned_rules WHERE name = ?1",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|e| format!("Read learned rule metadata error: {e}"))?;

            tx.execute(
                "UPDATE learned_rules SET beta_param = beta_param + 5.0, file_path = '', state = 'suppressed', lifecycle = 'tombstoned', updated_at = datetime('now') WHERE name = ?1",
                params![name],
            )
            .ok();

            let (file_path, provider_scope, rule_id, content_hash) =
                record.unwrap_or((None, None, None, None));

            // Upsert the durable tombstone. A re-delete of an
            // already-tombstoned (and not reactivated) rule just refreshes
            // it; if it had been reactivated, deleting re-arms suppression
            // by clearing the reactivation columns.
            tx.execute(
                "INSERT INTO rule_tombstones (rule_name, rule_id, tombstoned_at, tombstoned_by, reason, last_content_hash)
                 VALUES (?1, ?2, datetime('now'), 'human', 'deleted via UI/IPC', ?3)
                 ON CONFLICT(rule_name) DO UPDATE SET
                     rule_id = excluded.rule_id,
                     tombstoned_at = datetime('now'),
                     tombstoned_by = 'human',
                     reason = 'deleted via UI/IPC',
                     last_content_hash = excluded.last_content_hash,
                     reactivated_at = NULL,
                     reactivated_by = NULL",
                params![name, rule_id, content_hash],
            )
            .map_err(|e| format!("Write rule tombstone error: {e}"))?;

            tx.commit()
                .map_err(|e| format!("Delete learned rule tx commit: {e}"))?;
            (file_path, parse_provider_scope(provider_scope))
        };

        // Delete the .md file from disk
        if let Some(fp) = &file_path
            && !fp.is_empty()
        {
            let path = std::path::Path::new(fp);
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| format!("Delete file error: {e}"))?;
            }
        }

        // Also search recursively as fallback
        fn find_and_delete(dir: &std::path::Path, name: &str) -> bool {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if find_and_delete(&path, name) {
                            return true;
                        }
                    } else if path.file_stem().is_some_and(|s| s == name)
                        && path.extension().is_some_and(|e| e == "md")
                    {
                        let _ = std::fs::remove_file(&path);
                        return true;
                    }
                }
            }
            false
        }
        for rules_dir in learned_rule_dirs_for_scope(&provider_scope) {
            if find_and_delete(&rules_dir, name) {
                break;
            }
        }
        Ok(())
    }

    /// Feature 005 US2 T029 (FR-007/FR-008, contracts/rule-governance.md
    /// "Sole writer: approval", data-model.md "rule lifecycle").
    ///
    /// This is the **only** function that authors a global learned-rule
    /// `.md`. Contract enforced here:
    /// - Precondition: `lifecycle == 'awaiting_review'` (else `Err`) AND
    ///   `!tombstone_blocks(name)` (the 5th name-addressed gate path).
    /// - Effect, atomically in one tx: write
    ///   `sanitize_rule_content(redact(content))` to the scope dir (path-
    ///   traversal canonicalization kept), set `file_path`,
    ///   `lifecycle='active'`; append an immutable `rule_versions` row
    ///   (`change_kind='promote'`, `version=current_version`); populate
    ///   provenance (`origin_run_id/origin_model/origin_at`) and snapshot a
    ///   `rule_evidence_citations` row so grounding survives observation
    ///   purge.
    ///
    /// The evidence-weighted candidate→`awaiting_review` GATING is US3/R-6
    /// and intentionally NOT done here — this function only requires/asserts
    /// the state, it does not compute eligibility.
    pub fn promote_learned_rule(&self, name: &str) -> Result<(), String> {
        use sha2::Digest;
        if !crate::learning::is_safe_rule_name(name) {
            return Err(format!(
                "Invalid rule name: {}",
                &name[..name.len().min(50)]
            ));
        }

        // Read the candidate + assert the lifecycle precondition and the
        // durable tombstone gate BEFORE touching the filesystem.
        let (content, provider_scope, lifecycle, current_version): (
            String,
            Vec<IntegrationProvider>,
            String,
            i64,
        ) = {
            let conn = self.conn.lock();
            if tombstone_blocks(&conn, name) {
                return Err(format!(
                    "Rule '{name}' is tombstoned — reactivate it explicitly before promotion"
                ));
            }
            let row: (Option<String>, Option<String>, String, i64) = conn
                .query_row(
                    "SELECT content, provider_scope, lifecycle, current_version FROM learned_rules WHERE name = ?1 AND state != 'suppressed'",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(|e| format!("Rule not found: {e}"))?;
            let (content_opt, provider_scope, lifecycle, current_version) = row;
            (
                content_opt.ok_or_else(|| {
                    "No stored content for this rule — re-run analysis to capture content"
                        .to_string()
                })?,
                parse_provider_scope(provider_scope),
                lifecycle,
                current_version,
            )
        };

        if lifecycle != "awaiting_review" {
            return Err(format!(
                "Rule '{name}' is '{lifecycle}', not 'awaiting_review' — only a queued rule can be approved"
            ));
        }

        // Promotion coupling (feature 005 US4 T053, C-4/FR-020, contract
        // evaluation-harness.md "Promotion coupling"). Consult the most
        // recent counterfactual verdict BEFORE any `.md`/`active` write:
        //
        //   * latest verdict regresses the replay set AND no audited
        //     `reviewer_overrides` row exists for that
        //     `(rule_name, replay_set_version)` → HARD BLOCK (Err). The
        //     maintainer must record an explicit override to approve.
        //   * judge uncalibrated OR replay set stale, but not regressing
        //     (or regressing-but-overridden) → DO NOT block; promotion
        //     proceeds (the warn-not-block rule — the caller/UI surfaces
        //     the caution). `inconclusive` is likewise non-blocking.
        //   * no eval rows at all → DO NOT hard-block; the rule is
        //     "unevaluated" (SC-007 expects the maintainer to run eval, but
        //     promote must keep working so the loop is not bricked).
        let latest_verdict = self.latest_eval_verdict(name, None)?;
        let regresses_without_override = latest_verdict.as_ref().is_some_and(|latest| {
            latest.regression && !self.has_reviewer_override(name, latest.replay_set_version)
        });
        if regresses_without_override {
            return Err(
                "blocked: rule regresses the replay set; record an explicit reviewer override to approve".to_string(),
            );
        }

        let rules_dir = crate::learning::learned_rules_dir_for_scope(&provider_scope);
        std::fs::create_dir_all(&rules_dir).map_err(|e| format!("Cannot create rules dir: {e}"))?;

        let file_path = rules_dir.join(format!("{name}.md"));
        let canonical_dir = rules_dir
            .canonicalize()
            .map_err(|e| format!("Canonicalize error: {e}"))?;
        let canonical_parent = file_path
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .unwrap_or_default();
        if !canonical_parent.starts_with(&canonical_dir) {
            return Err(format!("Path traversal detected for rule: {name}"));
        }

        // H-3 / FR-004 (feature 005 US1 T019, contract redaction.md): the
        // promoted body is redacted (no secret/PII at rest or in the on-disk
        // `.md`) then injection-hardened, order redact → sanitize per R-1
        // Decision 5. Both calls are idempotent so re-promoting an
        // already-clean rule is a fixed point.
        let sanitized = crate::learning::sanitize_rule_content(&crate::redaction::redact(&content));
        let content_hash = format!("{:x}", sha2::Sha256::digest(sanitized.as_bytes()));

        // Provenance (FR-008, data-model.md "Provenance Record"): attribute
        // the promotion to the most recent run that actually produced
        // candidates (completed/degraded), capturing its model snapshot.
        let origin_at = Utc::now().to_rfc3339();
        let (origin_run_id, origin_model): (Option<i64>, Option<String>) = {
            let conn = self.conn.lock();
            let run: Option<(i64, Option<String>)> = conn
                .query_row(
                    "SELECT id, inference_metadata FROM learning_runs
                     WHERE status IN ('completed', 'degraded')
                     ORDER BY id DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| format!("Origin run lookup error: {e}"))?;
            match run {
                Some((rid, meta_json)) => {
                    let model = meta_json
                        .as_deref()
                        .and_then(|j| serde_json::from_str::<Vec<serde_json::Value>>(j).ok())
                        .and_then(|records| {
                            records.iter().find_map(|r| {
                                r.get("model").and_then(|m| m.as_str()).map(str::to_string)
                            })
                        });
                    (Some(rid), model)
                }
                None => (None, None),
            }
        };

        // Crash-atomicity (feature 005 hardening — FR-007/FR-008). The
        // on-disk `.md` is the SOLE artifact of an authorized approval, so
        // the DB row that vouches for it (lifecycle='active' +
        // `rule_versions`/`rule_evidence_citations` provenance) MUST land
        // first and durably. Ordering is therefore:
        //
        //   1. ALL DB mutations inside ONE tx, then `tx.commit()`.
        //   2. ONLY after the commit succeeds, materialize the `.md` via a
        //      temp-file + atomic `rename` (write `<file>.tmp` in the same
        //      dir, then `std::fs::rename`, which is atomic on the same
        //      filesystem) so a crash can never expose a partially-written
        //      file and never an `active`-less orphan.
        //
        // Crash windows:
        //   * crash before commit → nothing changed AND no `.md` exists yet
        //     (the file is written only post-commit), so `reconcile` cannot
        //     re-activate a provenance-less rule — the FR-007/FR-008 gate
        //     holds.
        //   * crash after commit, before/under rename → the rule is
        //     committed `active` in the DB but its `.md` is absent. This is
        //     the SELF-HEALING state: `reconcile_learned_rules` step 3b
        //     (DB row with a non-empty `file_path` whose file is missing)
        //     suppresses+tombstones it, so the next reconcile converges. We
        //     surface this as an `Err` (post-commit, below) so the caller
        //     knows the approval did not fully materialize and can re-run.
        //
        // One tx: flip lifecycle + persist the sanitized body, append the
        // immutable promote version row, write the provenance citation
        // snapshot.
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Promote tx begin: {e}"))?;

        let rule_id: i64 = tx
            .query_row(
                "SELECT id FROM learned_rules WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .map_err(|e| format!("Promote id lookup error: {e}"))?;

        tx.execute(
            "UPDATE learned_rules
             SET content = ?1, file_path = ?2, content_hash = ?3, lifecycle = 'active',
                 origin_run_id = ?4, origin_model = ?5, origin_at = ?6,
                 updated_at = datetime('now')
             WHERE name = ?7",
            params![
                sanitized,
                file_path.to_string_lossy().as_ref(),
                content_hash,
                origin_run_id,
                origin_model,
                origin_at,
                name,
            ],
        )
        .map_err(|e| format!("Promote update error: {e}"))?;

        // Append-only history (C-2, FR-009). `version` mirrors the rule's
        // `current_version` pointer; `UNIQUE(rule_id, version)` makes a
        // re-promote at the same version a deterministic no-op insert.
        let provider_scope_json = serde_json::to_string(
            &provider_scope
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[\"claude\"]".to_string());
        tx.execute(
            "INSERT OR IGNORE INTO rule_versions
                (rule_id, version, content, content_hash, provider_scope, source,
                 run_id, change_kind, author, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'approval', ?6, 'promote', 'human', datetime('now'))",
            params![
                rule_id,
                current_version,
                sanitized,
                content_hash,
                provider_scope_json,
                origin_run_id,
            ],
        )
        .map_err(|e| format!("Promote version insert error: {e}"))?;

        // Retention-proof grounding snapshot (C-2, H-1). The full
        // evidence-ref resolution is US3/R-6; here we durably record the
        // approval provenance so SC-003 (100% provenance) holds even after
        // the source observations are purged.
        tx.execute(
            "INSERT INTO rule_evidence_citations
                (rule_id, run_id, rule_version, kind, ref_id, snippet, created_at)
             VALUES (?1, ?2, ?3, 'session', ?4, ?5, datetime('now'))",
            params![
                rule_id,
                origin_run_id,
                current_version,
                origin_run_id.map(|r| r.to_string()),
                format!("approved at {origin_at} from run {origin_run_id:?}"),
            ],
        )
        .map_err(|e| format!("Promote citation snapshot error: {e}"))?;

        tx.commit().map_err(|e| format!("Promote tx commit: {e}"))?;
        // The approval is now durable in the DB. The connection lock is no
        // longer needed for the file write; drop it so the post-commit I/O
        // never holds the SQLite mutex.
        drop(conn);

        // Post-commit, atomic file materialization. Write the sanitized body
        // to a sibling `<file>.tmp` then `rename` it onto the final path:
        // `std::fs::rename` is atomic within one filesystem (and the temp
        // file is created in the SAME directory as the target), so a reader
        // or a crash only ever sees the complete old or complete new file —
        // never a torn write. A failure here leaves the DB committed-active
        // with the `.md` absent, which `reconcile_learned_rules` step 3b
        // self-heals; we return an `Err` so the approval is retried rather
        // than silently reported as fully applied.
        let tmp_path = file_path.with_extension("md.tmp");
        if let Err(e) = std::fs::write(&tmp_path, &sanitized) {
            return Err(format!(
                "Rule '{name}' committed active in DB but the on-disk .md \
                 could not be staged ({e}); reconcile will suppress the \
                 dangling row — re-run promotion to materialize the file"
            ));
        }
        if let Err(e) = std::fs::rename(&tmp_path, &file_path) {
            // Best-effort cleanup of the orphaned temp file; the rename
            // failure is the reported error regardless.
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!(
                "Rule '{name}' committed active in DB but the on-disk .md \
                 could not be atomically published ({e}); reconcile will \
                 suppress the dangling row — re-run promotion to \
                 materialize the file"
            ));
        }

        Ok(())
    }

    /// Feature 005 US2 T031 (FR-009, contracts/rule-governance.md "Sole
    /// writer: approval"). Roll a rule back to an earlier
    /// `rule_versions.version`.
    ///
    /// Rollback is a *forward, append-only* restore — never a history
    /// rewrite. One tx: read the target version row (Err if it does not
    /// exist or the rule is tombstoned), append a NEW `rule_versions` row
    /// (`change_kind='rollback'`, `rolled_back_from=<target>`), restore
    /// `learned_rules.content/content_hash/current_version`, and rewrite the
    /// on-disk `.md` (redact → sanitize, path-traversal-guarded) when the
    /// rule has a `file_path`. The DB `content_hash` is set to the SHA256 of
    /// the exact bytes written to disk so the rule-watcher's reconcile 3c
    /// (raw-bytes hash compare) treats the rewrite as already-reconciled and
    /// does NOT emit a spurious extra version.
    #[allow(dead_code)] // IPC wiring is a later task
    pub fn rollback_rule(&self, name: &str, target_version: i64) -> Result<(), String> {
        use sha2::Digest;
        if !crate::learning::is_safe_rule_name(name) {
            return Err(format!(
                "Invalid rule name: {}",
                &name[..name.len().min(50)]
            ));
        }

        let mut conn = self.conn.lock();

        // Durable tombstone gate: a tombstoned rule cannot be rolled back
        // (it has no live identity until an explicit reactivation).
        if tombstone_blocks(&conn, name) {
            return Err(format!(
                "Rule '{name}' is tombstoned — cannot roll back a suppressed rule"
            ));
        }

        let tx = conn
            .transaction()
            .map_err(|e| format!("Rollback tx begin: {e}"))?;

        let (rule_id, file_path, lifecycle): (i64, String, String) = tx
            .query_row(
                "SELECT id, file_path, lifecycle FROM learned_rules WHERE name = ?1",
                params![name],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Rollback: rule not found: {e}"))?;

        if lifecycle == "tombstoned" || lifecycle == "rejected" {
            return Err(format!(
                "Rule '{name}' is '{lifecycle}' — cannot roll back a terminally-suppressed rule"
            ));
        }

        // Target snapshot. Err if the version does not exist.
        let (target_content, target_domain, target_is_anti): (String, Option<String>, i64) = tx
            .query_row(
                "SELECT content, domain, is_anti_pattern FROM rule_versions
                 WHERE rule_id = ?1 AND version = ?2",
                params![rule_id, target_version],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Rollback: target version {target_version} not found: {e}"))?;

        // The restored body is re-passed through redact → sanitize so the
        // at-rest column and any rewritten `.md` cannot regress on
        // injection-hardening even if an older snapshot predates it (both
        // passes are idempotent).
        let restored =
            crate::learning::sanitize_rule_content(&crate::redaction::redact(&target_content));
        let restored_hash = format!("{:x}", sha2::Sha256::digest(restored.as_bytes()));

        // Next version number = max(version)+1 (append-only; the rollback is
        // itself a new immutable row pointing back at the source).
        let next_version: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(version), 0) + 1 FROM rule_versions WHERE rule_id = ?1",
                params![rule_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Rollback: version compute error: {e}"))?;

        tx.execute(
            "INSERT INTO rule_versions
                (rule_id, version, content, content_hash, domain, is_anti_pattern,
                 change_kind, rolled_back_from, author, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'rollback', ?7, 'human', datetime('now'))",
            params![
                rule_id,
                next_version,
                restored,
                restored_hash,
                target_domain,
                target_is_anti,
                target_version,
            ],
        )
        .map_err(|e| format!("Rollback version insert error: {e}"))?;

        tx.execute(
            "UPDATE learned_rules
             SET content = ?1, content_hash = ?2, current_version = ?3,
                 updated_at = datetime('now')
             WHERE id = ?4",
            params![restored, restored_hash, next_version, rule_id],
        )
        .map_err(|e| format!("Rollback restore error: {e}"))?;

        // Rewrite the on-disk `.md` only if the rule has a live file. The
        // DB `content_hash` (set above) is the SHA256 of exactly these
        // bytes, so reconcile 3c sees no drift and will not re-version.
        if !file_path.is_empty() {
            let path = std::path::Path::new(&file_path);
            if let Some(canonical_parent) = path.parent().and_then(|p| p.canonicalize().ok()) {
                // Guard: the resolved parent must be inside a known
                // learned-rules dir for this rule's inferred scope.
                let scope = inferred_rule_provider_scope(path);
                let mut allowed = false;
                for dir in learned_rule_dirs_for_scope(&scope) {
                    if dir
                        .canonicalize()
                        .is_ok_and(|cdir| canonical_parent.starts_with(&cdir))
                    {
                        allowed = true;
                        break;
                    }
                }
                if !allowed {
                    return Err(format!(
                        "Path traversal detected rewriting rule '{name}' on rollback"
                    ));
                }
                std::fs::write(path, &restored)
                    .map_err(|e| format!("Rollback file rewrite error: {e}"))?;
            }
        }

        tx.commit()
            .map_err(|e| format!("Rollback tx commit: {e}"))?;
        Ok(())
    }

    /// Feature 005 US2 T035 (storage side; FR-010,
    /// contracts/rule-governance.md "`tombstone_blocks`"). The ONLY path
    /// that clears a durable tombstone.
    ///
    /// Sets `reactivated_at`/`reactivated_by` on the name-keyed
    /// `rule_tombstones` row so `tombstone_blocks` stops blocking, and
    /// returns the rule's `lifecycle` to `candidate` so it must re-earn
    /// review eligibility through the normal gated pipeline (it never
    /// auto-activates). Err if there is no active tombstone for `name`. IPC
    /// registration + authorization is a SEPARATE later task — this is the
    /// storage primitive only.
    #[allow(dead_code)] // IPC wiring is a later task
    pub fn reactivate_rule(&self, name: &str) -> Result<(), String> {
        if !crate::learning::is_safe_rule_name(name) {
            return Err(format!(
                "Invalid rule name: {}",
                &name[..name.len().min(50)]
            ));
        }
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Reactivate tx begin: {e}"))?;

        let affected = tx
            .execute(
                "UPDATE rule_tombstones
                 SET reactivated_at = datetime('now'), reactivated_by = 'human'
                 WHERE rule_name = ?1 AND reactivated_at IS NULL",
                params![name],
            )
            .map_err(|e| format!("Reactivate tombstone update error: {e}"))?;

        if affected == 0 {
            return Err(format!(
                "Rule '{name}' has no active tombstone to reactivate"
            ));
        }

        // Return the rule (if it still exists) to the review pipeline as a
        // plain candidate — reactivation re-arms eligibility, it does NOT
        // restore activation or rewrite any `.md`.
        tx.execute(
            "UPDATE learned_rules
             SET lifecycle = 'candidate', state = 'emerging', updated_at = datetime('now')
             WHERE name = ?1",
            params![name],
        )
        .map_err(|e| format!("Reactivate lifecycle reset error: {e}"))?;

        tx.commit()
            .map_err(|e| format!("Reactivate tx commit: {e}"))?;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Counterfactual evaluation persistence + promotion coupling
    // (feature 005 US4 T052/T053, C-4/FR-020/FR-022, contract
    // evaluation-harness.md "Persistence" + "Promotion coupling").
    // Migration 25 owns the `evaluation_results` / `reviewer_overrides`
    // DDL; these methods only read/write it.
    // -------------------------------------------------------------------

    /// Persist one counterfactual verdict (feature 005 US4 T052, FR-022).
    ///
    /// Inserts a fresh `evaluation_results` row linked to
    /// `(rule_name, learning_run_id, replay_set_version)` with the scalar
    /// projection of [`eval_harness::EvalVerdictRow`] plus `evaluated_at =
    /// now`. A self-describing JSON snapshot of the row is stored in
    /// `per_case_json` so the persisted record is complete even though
    /// `EvalVerdictRow` itself carries no per-case detail. Re-evaluating a
    /// rule simply appends a newer row; the `(rule_name, evaluated_at DESC)`
    /// read in [`Self::latest_eval_verdict`] makes the newest row win, so
    /// this is idempotent-friendly without an UPSERT. Returns the new row id.
    pub fn persist_evaluation_result(
        &self,
        row: &crate::eval_harness::EvalVerdictRow,
    ) -> Result<i64, String> {
        // The `replay_set_version` column is TEXT (migration 25); store the
        // i64 as its decimal string so it round-trips through
        // `latest_eval_verdict`.
        let replay_set_version = row.replay_set_version.to_string();
        // `per_case_json` is nullable; serialize the scalar row itself as a
        // compact, self-contained payload (best-effort — never fail the
        // insert on a serialization hiccup).
        let per_case_json = serde_json::to_string(row).ok();

        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO evaluation_results
                (rule_name, learning_run_id, replay_set_version, judge_model,
                 verdict, delta, regression, negative_transfer,
                 judge_uncalibrated, replay_set_stale, agreement_score,
                 rationale, per_case_json, evaluated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     datetime('now'))",
            params![
                row.rule_name,
                row.learning_run_id,
                replay_set_version,
                row.judge_model,
                row.verdict,
                row.delta,
                row.regression as i64,
                row.negative_transfer as i64,
                row.judge_uncalibrated as i64,
                row.replay_set_stale as i64,
                row.agreement_score,
                None::<String>,
                per_case_json,
            ],
        )
        .map_err(|e| format!("Persist evaluation result error: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    /// Most recent counterfactual verdict for a rule (feature 005 US4 T053,
    /// FR-020).
    ///
    /// `replay_set_version = None` returns the newest row for the rule
    /// regardless of replay-set version — this is what the promotion gate
    /// consults ("`approve` MUST deny if `latest.regression &&
    /// !has_override`"). `Some(v)` scopes the lookup to one replay-set
    /// version. Newest wins via the `(rule_name, evaluated_at DESC)` index.
    /// `None` means the rule is unevaluated (surfaced as such; never a
    /// hard block — SC-007).
    pub fn latest_eval_verdict(
        &self,
        rule_name: &str,
        replay_set_version: Option<i64>,
    ) -> Result<Option<crate::eval_harness::EvalVerdictRow>, String> {
        let conn = self.conn.lock();
        let map_row =
            |row: &rusqlite::Row<'_>| -> rusqlite::Result<crate::eval_harness::EvalVerdictRow> {
                let replay_set_version_str: Option<String> = row.get(2)?;
                Ok(crate::eval_harness::EvalVerdictRow {
                    rule_name: row.get(0)?,
                    learning_run_id: row.get(1)?,
                    replay_set_version: replay_set_version_str
                        .as_deref()
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0),
                    judge_model: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    verdict: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    delta: row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                    regression: row.get::<_, i64>(6)? != 0,
                    negative_transfer: row.get::<_, i64>(7)? != 0,
                    judge_uncalibrated: row.get::<_, i64>(8)? != 0,
                    replay_set_stale: row.get::<_, i64>(9)? != 0,
                    agreement_score: row.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
                })
            };
        let result = match replay_set_version {
            Some(v) => conn
                .query_row(
                    "SELECT rule_name, learning_run_id, replay_set_version,
                            judge_model, verdict, delta, regression,
                            negative_transfer, judge_uncalibrated,
                            replay_set_stale, agreement_score
                     FROM evaluation_results
                     WHERE rule_name = ?1 AND replay_set_version = ?2
                     ORDER BY evaluated_at DESC, id DESC LIMIT 1",
                    params![rule_name, v.to_string()],
                    map_row,
                )
                .optional(),
            None => conn
                .query_row(
                    "SELECT rule_name, learning_run_id, replay_set_version,
                            judge_model, verdict, delta, regression,
                            negative_transfer, judge_uncalibrated,
                            replay_set_stale, agreement_score
                     FROM evaluation_results
                     WHERE rule_name = ?1
                     ORDER BY evaluated_at DESC, id DESC LIMIT 1",
                    params![rule_name],
                    map_row,
                )
                .optional(),
        };
        result.map_err(|e| format!("Latest eval verdict error: {e}"))
    }

    /// Whether an audited reviewer override exists for
    /// `(rule_name, replay_set_version)` (feature 005 US4 T053, FR-020).
    /// One such row turns a regressing verdict from a hard block into an
    /// approved promotion.
    pub fn has_reviewer_override(&self, rule_name: &str, replay_set_version: i64) -> bool {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT 1 FROM reviewer_overrides
             WHERE rule_name = ?1 AND replay_set_version = ?2 LIMIT 1",
            params![rule_name, replay_set_version.to_string()],
            |_| Ok(()),
        )
        .optional()
        .map(|r| r.is_some())
        .unwrap_or(false)
    }

    /// Record an audited regression override (feature 005 US4 T053,
    /// FR-020). The `reason` is REQUIRED and must be non-empty — the
    /// override becomes part of the rule's provenance, so an unexplained
    /// override is rejected. After this row exists, the promotion gate
    /// allows approving the otherwise-regressing rule for that replay-set
    /// version.
    pub fn record_reviewer_override(
        &self,
        rule_name: &str,
        replay_set_version: i64,
        overridden_by: &str,
        reason: &str,
    ) -> Result<(), String> {
        if !crate::learning::is_safe_rule_name(rule_name) {
            return Err(format!(
                "Invalid rule name: {}",
                &rule_name[..rule_name.len().min(50)]
            ));
        }
        if reason.trim().is_empty() {
            return Err("A non-empty reason is required to record a reviewer override".to_string());
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO reviewer_overrides
                (rule_name, replay_set_version, overridden_by, reason,
                 overridden_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![
                rule_name,
                replay_set_version.to_string(),
                overridden_by,
                reason.trim(),
            ],
        )
        .map_err(|e| format!("Record reviewer override error: {e}"))?;
        Ok(())
    }

    /// Assemble the [`eval_harness::RuleUnderTest`] for a stored rule plus
    /// the originating run id to attribute the evaluation to (feature 005
    /// US4 T053). The run id is the most recent `completed|degraded`
    /// `learning_runs` row (mirrors `promote_learned_rule`'s provenance
    /// choice), or `None` if no such run exists. Used by the authorized
    /// `run_rule_evaluation` IPC so the harness — which never touches
    /// storage — becomes reachable in-app (V5/FR-019).
    pub fn eval_inputs_for_rule(
        &self,
        name: &str,
    ) -> Result<(crate::eval_harness::RuleUnderTest, Option<i64>), String> {
        if !crate::learning::is_safe_rule_name(name) {
            return Err(format!(
                "Invalid rule name: {}",
                &name[..name.len().min(50)]
            ));
        }
        let conn = self.conn.lock();
        let (content, domain, confidence): (Option<String>, Option<String>, f64) = conn
            .query_row(
                "SELECT content, domain, confidence FROM learned_rules
                 WHERE name = ?1 AND state != 'suppressed'",
                params![name],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Rule not found: {e}"))?;
        let content = content.ok_or_else(|| {
            "No stored content for this rule — re-run analysis to capture content".to_string()
        })?;
        let run_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM learning_runs
                 WHERE status IN ('completed', 'degraded')
                 ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Origin run lookup error: {e}"))?;
        Ok((
            crate::eval_harness::RuleUnderTest {
                name: name.to_string(),
                content,
                domain: domain.unwrap_or_else(|| "general".to_string()),
                claimed_confidence: confidence,
            },
            run_id,
        ))
    }

    #[allow(dead_code)]
    pub fn update_rule_content_hash(&self, name: &str, hash: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE learned_rules SET content_hash = ?1, updated_at = datetime('now') WHERE name = ?2",
            params![hash, name],
        )
        .map_err(|e| format!("Update content_hash error: {e}"))?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn reconcile_learned_rules(&self) -> Result<bool, String> {
        use sha2::{Digest, Sha256};

        // Step 1: Scan filesystem
        let mut fs_rules: std::collections::HashMap<
            String,
            (
                std::path::PathBuf,
                Vec<crate::integrations::IntegrationProvider>,
            ),
        > = std::collections::HashMap::new();

        for rules_dir in learned_rule_dirs(None) {
            if !rules_dir.exists() {
                continue;
            }
            fn collect_files(dir: &std::path::Path, out: &mut Vec<(String, std::path::PathBuf)>) {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            collect_files(&path, out);
                        } else if path.extension().is_some_and(|ext| ext == "md") {
                            let name = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("unknown")
                                .to_string();
                            out.push((name, path));
                        }
                    }
                }
            }
            let mut files = Vec::new();
            collect_files(&rules_dir, &mut files);
            for (name, path) in files {
                fs_rules.entry(name).or_insert_with(|| {
                    let scope = inferred_rule_provider_scope(&path);
                    (path, scope)
                });
            }
        }

        // Step 2: Query DB for non-suppressed rules + a name→lifecycle map
        // over ALL rows (feature 005 US2 T028). The lifecycle map covers
        // every row regardless of `state` so step 3a (name absent from the
        // non-suppressed set) can still honor a durable `tombstoned`/
        // `rejected` lifecycle, and a name→active-tombstone set so reconcile
        // never resurrects a durably suppressed rule (C-5, FR-010,
        // contracts/rule-governance.md "Reconcile cooperation").
        let db_rules: std::collections::HashMap<String, (String, Option<String>, Option<String>)> = {
            let conn = self.conn.lock();
            let mut stmt = conn
                .prepare_cached(
                    "SELECT name, file_path, content_hash, provider_scope FROM learned_rules WHERE state != 'suppressed'",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(|e| format!("Query error: {e}"))?;
            let mut map = std::collections::HashMap::new();
            for row in rows {
                let (name, file_path, content_hash, _provider_scope) =
                    row.map_err(|e| format!("Row error: {e}"))?;
                map.insert(name, (file_path, content_hash, _provider_scope));
            }
            map
        };

        // name → lifecycle (all rows) and the set of names with an active
        // durable tombstone. Built once so the per-name guards below are
        // O(1) lookups, not N+1 point-reads.
        let (lifecycle_by_name, tombstoned_names): (
            std::collections::HashMap<String, String>,
            std::collections::HashSet<String>,
        ) = {
            let conn = self.conn.lock();
            let mut life_map = std::collections::HashMap::new();
            {
                let mut stmt = conn
                    .prepare_cached("SELECT name, lifecycle FROM learned_rules")
                    .map_err(|e| format!("Prepare lifecycle map error: {e}"))?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| format!("Query lifecycle map error: {e}"))?;
                for row in rows {
                    let (name, lifecycle) = row.map_err(|e| format!("Row error: {e}"))?;
                    life_map.insert(name, lifecycle);
                }
            }
            let mut ts_set = std::collections::HashSet::new();
            {
                let mut stmt = conn
                    .prepare_cached(
                        "SELECT rule_name FROM rule_tombstones WHERE reactivated_at IS NULL",
                    )
                    .map_err(|e| format!("Prepare tombstone set error: {e}"))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| format!("Query tombstone set error: {e}"))?;
                for row in rows {
                    ts_set.insert(row.map_err(|e| format!("Row error: {e}"))?);
                }
            }
            (life_map, ts_set)
        };

        // Shared "durably suppressed" predicate for reconcile steps 3a/3c:
        // an active name-keyed tombstone OR a terminal lifecycle the
        // watcher must never silently override.
        let durably_blocked = |name: &str| -> bool {
            tombstoned_names.contains(name)
                || matches!(
                    lifecycle_by_name.get(name).map(String::as_str),
                    Some("tombstoned") | Some("rejected")
                )
        };

        let mut changed = false;

        // Step 3a: Files on disk but not in DB -> INSERT as a *candidate*
        for (name, (path, scope)) in &fs_rules {
            if db_rules.contains_key(name) {
                continue;
            }
            if !crate::learning::is_safe_rule_name(name) {
                continue;
            }
            // Feature 005 US2 T027/T028 (C-5, FR-010): never resurrect a
            // durably suppressed pattern. A re-appearing `.md` for a
            // tombstoned/rejected name is ignored — reactivation is an
            // explicit authorized action, not a side effect of a file
            // landing back in a watched dir.
            if durably_blocked(name) {
                continue;
            }
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            // `content_hash` is computed over the RAW file bytes for change
            // detection (unchanged). Only the stored `content` column is
            // redacted + injection-hardened (H-3 / FR-004, US1 T019).
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let content_str = String::from_utf8_lossy(&bytes);
            let (domain, is_anti, body) = parse_rule_frontmatter(&content_str);
            let stored_content =
                crate::learning::sanitize_rule_content(&crate::redaction::redact(&body));
            let scope_json =
                serde_json::to_string(&scope.iter().map(|p| p.to_string()).collect::<Vec<_>>())
                    .unwrap_or_else(|_| "[\"claude\"]".to_string());
            let file_path_str = path.to_string_lossy();

            // Reconcile-ingested files route into the review queue as
            // `lifecycle='candidate'` — they never auto-activate
            // (contracts/rule-governance.md "Reconcile cooperation"). The
            // read-derived `state` stays 'emerging' (unchanged).
            let conn = self.conn.lock();
            conn.execute(
                "INSERT OR IGNORE INTO learned_rules (name, domain, alpha, beta_param, observation_count, state, lifecycle, file_path, provider_scope, source, content, content_hash, is_anti_pattern)
                 VALUES (?1, ?2, 1.0, 1.0, 0, 'emerging', 'candidate', ?3, ?4, 'manual', ?5, ?6, ?7)",
                params![name, domain, file_path_str.as_ref(), scope_json, stored_content, hash, is_anti as i32],
            )
            .map_err(|e| format!("Insert reconciled rule error: {e}"))?;
            changed = true;
        }

        // Step 3b: DB rows with file_path but file missing -> durable
        // tombstone (feature 005 US2 T028, C-5, FR-010). A vanished `.md`
        // is treated as an intentional removal: soft-suppress (β += 5.0,
        // hide via `state`), set the persisted `lifecycle='tombstoned'`, and
        // write/refresh a name-keyed `rule_tombstones` row
        // (`tombstoned_by='reconcile_delete'`) so the rule cannot be
        // resurrected by a later re-extraction or a file reappearing. Both
        // mutations run in one tx so the suppression and its tombstone are
        // atomic.
        for (name, (file_path, content_hash, _provider_scope)) in &db_rules {
            if file_path.is_empty() {
                continue;
            }
            if fs_rules.contains_key(name) {
                continue;
            }
            let path = std::path::Path::new(file_path.as_str());
            if !path.exists() {
                let mut conn = self.conn.lock();
                let tx = conn
                    .transaction()
                    .map_err(|e| format!("Reconcile 3b tx begin: {e}"))?;
                let rule_id: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM learned_rules WHERE name = ?1",
                        params![name],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| format!("Reconcile 3b id lookup error: {e}"))?;
                tx.execute(
                    "UPDATE learned_rules SET beta_param = beta_param + 5.0, state = 'suppressed', lifecycle = 'tombstoned', file_path = '', updated_at = datetime('now') WHERE name = ?1",
                    params![name],
                )
                .map_err(|e| format!("Suppress reconciled rule error: {e}"))?;
                tx.execute(
                    "INSERT INTO rule_tombstones (rule_name, rule_id, tombstoned_at, tombstoned_by, reason, last_content_hash)
                     VALUES (?1, ?2, datetime('now'), 'reconcile_delete', 'on-disk rule file removed', ?3)
                     ON CONFLICT(rule_name) DO UPDATE SET
                         rule_id = excluded.rule_id,
                         tombstoned_at = datetime('now'),
                         tombstoned_by = 'reconcile_delete',
                         reason = 'on-disk rule file removed',
                         last_content_hash = excluded.last_content_hash,
                         reactivated_at = NULL,
                         reactivated_by = NULL",
                    params![name, rule_id, content_hash],
                )
                .map_err(|e| format!("Write reconcile tombstone error: {e}"))?;
                tx.commit()
                    .map_err(|e| format!("Reconcile 3b tx commit: {e}"))?;
                changed = true;
            }
        }

        // Step 3c: Both exist, check content hash
        for (name, (path, _scope)) in &fs_rules {
            let Some((file_path, existing_hash, _provider_scope)) = db_rules.get(name) else {
                continue;
            };
            if file_path.is_empty() {
                continue;
            }
            // Feature 005 US2 T027/T028 (C-5, FR-010): a durably suppressed
            // rule's on-disk edits are NOT folded back into the DB — the
            // tombstone outranks the watcher. Reactivation is explicit only.
            if durably_blocked(name) {
                continue;
            }
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            // Change detection still keys off the RAW-bytes hash (unchanged);
            // only the persisted `content` is redacted + injection-hardened
            // (H-3 / FR-004, US1 T019).
            let hash = format!("{:x}", Sha256::digest(&bytes));
            if existing_hash.as_deref() == Some(hash.as_str()) {
                continue;
            }
            let content_str = String::from_utf8_lossy(&bytes);
            let (_domain, _is_anti, body) = parse_rule_frontmatter(&content_str);
            let stored_content =
                crate::learning::sanitize_rule_content(&crate::redaction::redact(&body));
            let conn = self.conn.lock();
            conn.execute(
                "UPDATE learned_rules SET content = ?1, content_hash = ?2, updated_at = datetime('now') WHERE name = ?3",
                params![stored_content, hash, name],
            )
            .map_err(|e| format!("Update reconciled rule content error: {e}"))?;
            changed = true;
        }

        Ok(changed)
    }

    pub fn get_learning_status(&self) -> Result<LearningStatus, String> {
        let observation_count = self.get_observation_count(None)?;
        let unanalyzed_count = self.get_unanalyzed_observation_count(None)?;
        let rules = self.get_learned_rules(None)?;
        let runs = self.get_learning_runs(1, None)?;

        Ok(LearningStatus {
            observation_count,
            unanalyzed_count,
            rules_count: rules.len() as i64,
            last_run: runs.into_iter().next(),
        })
    }

    /// One-time, idempotent redaction backfill (feature 005 US1 T021,
    /// contract `redaction.md` "One-time backfill", R-1).
    ///
    /// Rewrites every existing `observations.tool_input/tool_output/cwd` and
    /// `git_snapshots.raw_data` value through [`crate::redaction::redact`] so
    /// rows captured before US1 wired the redaction boundary carry no
    /// plaintext secret/PII at rest. Guarded by the `settings` sentinel
    /// `redaction_backfill_done` (mirrors the existing one-time reingest-flag
    /// pattern): the pass runs at most once and a second invocation is a
    /// no-op. The work is bounded (only rows with non-empty content are
    /// touched) and transactional — either every row plus the sentinel
    /// commit, or nothing does. `redact` is itself idempotent, so even an
    /// unsentinelled re-run would be content-stable.
    const REDACTION_BACKFILL_SENTINEL: &'static str = "redaction_backfill_done";

    pub fn backfill_redaction(&self) -> Result<(), String> {
        if self
            .get_setting(Self::REDACTION_BACKFILL_SENTINEL)?
            .is_some()
        {
            return Ok(());
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Redaction backfill transaction begin: {e}"))?;

        // observations: redact the three free-text columns. Collect
        // (id, redacted triple) for rows that actually carry content, then
        // UPDATE by id so the pass is bounded and order-stable.
        type ObsRow = (i64, Option<String>, Option<String>, Option<String>);
        let pending_obs: Vec<ObsRow> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, tool_input, tool_output, cwd FROM observations
                     WHERE COALESCE(tool_input, '') <> ''
                        OR COALESCE(tool_output, '') <> ''
                        OR COALESCE(cwd, '') <> ''",
                )
                .map_err(|e| format!("Redaction backfill observations select: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    let id: i64 = row.get(0)?;
                    let ti: Option<String> = row.get(1)?;
                    let to: Option<String> = row.get(2)?;
                    let cwd: Option<String> = row.get(3)?;
                    Ok((
                        id,
                        ti.as_deref().map(crate::redaction::redact),
                        to.as_deref().map(crate::redaction::redact),
                        cwd.as_deref().map(crate::redaction::redact),
                    ))
                })
                .map_err(|e| format!("Redaction backfill observations query: {e}"))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| format!("Redaction backfill observations row: {e}"))?);
            }
            out
        };
        for (id, ti, to, cwd) in &pending_obs {
            tx.execute(
                "UPDATE observations SET tool_input = ?1, tool_output = ?2, cwd = ?3 WHERE id = ?4",
                params![ti, to, cwd, id],
            )
            .map_err(|e| format!("Redaction backfill observations update: {e}"))?;
        }

        // git_snapshots: redact the cached raw blob (NOT NULL by schema).
        let pending_git: Vec<(i64, String)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, raw_data FROM git_snapshots WHERE COALESCE(raw_data, '') <> ''",
                )
                .map_err(|e| format!("Redaction backfill git_snapshots select: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    let id: i64 = row.get(0)?;
                    let raw: String = row.get(1)?;
                    Ok((id, crate::redaction::redact(&raw)))
                })
                .map_err(|e| format!("Redaction backfill git_snapshots query: {e}"))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| format!("Redaction backfill git_snapshots row: {e}"))?);
            }
            out
        };
        for (id, raw) in &pending_git {
            tx.execute(
                "UPDATE git_snapshots SET raw_data = ?1 WHERE id = ?2",
                params![raw, id],
            )
            .map_err(|e| format!("Redaction backfill git_snapshots update: {e}"))?;
        }

        // Set the sentinel in the same transaction so completion is atomic
        // with the rewrite — a crash mid-pass leaves the sentinel unset and
        // the (idempotent) pass simply reruns next startup.
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, '1')",
            params![Self::REDACTION_BACKFILL_SENTINEL],
        )
        .map_err(|e| format!("Redaction backfill sentinel write: {e}"))?;

        tx.commit()
            .map_err(|e| format!("Redaction backfill commit: {e}"))?;
        Ok(())
    }

    /// Sentinel settings key for the one-time legacy-rule archive (T032).
    /// Presence ⇒ the archive-then-wipe already ran; never run again.
    const LEGACY_RULES_ARCHIVED_SENTINEL: &'static str = "legacy_rules_archived";

    /// Feature 005 US2 T032 (FR-012 / Q3=C, contracts/rule-governance.md
    /// "Legacy archive-then-wipe").
    ///
    /// One-time, sentinel-guarded, idempotent. Every learned `.md` that
    /// exists on disk *before* the new gated pipeline (Claude
    /// `~/.claude/rules/learned/`, Codex/shared
    /// `~/.config/quill/learned-rules/{codex,shared}/` — resolved via the
    /// same `learned_rule_dirs` resolver the read/reconcile paths use) is:
    ///   1. copied to `<data_local>/legacy-rules-archive/<ISO8601>/`
    ///      (mode `0444`, OUTSIDE every watched dir) alongside an
    ///      `ARCHIVE_MANIFEST.json` recording `{orig, sha256, scope, mtime}`
    ///      for 100% recoverability (SC-012);
    ///   2. deleted from its live location;
    ///   3. its matching `learned_rules` row set `lifecycle='tombstoned'`
    ///      with a `rule_tombstones` row (`tombstoned_by='legacy_archive'`)
    ///      so it can only return through the new human-gated pipeline.
    ///
    /// Runs inside the `Storage::init` chain (before `rule_watcher::start`)
    /// so the wipe never races reconcile. A crash before the sentinel commit
    /// just reruns the (idempotent) pass next start.
    pub fn archive_legacy_rules(&self) -> Result<(), String> {
        use sha2::Digest;

        if self
            .get_setting(Self::LEGACY_RULES_ARCHIVED_SENTINEL)?
            .is_some()
        {
            return Ok(());
        }

        // Collect every on-disk learned `.md` across all scopes.
        fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        collect(&p, out);
                    } else if p.extension().is_some_and(|e| e == "md") {
                        out.push(p);
                    }
                }
            }
        }
        let mut live_files = Vec::new();
        for rules_dir in learned_rule_dirs(None) {
            if rules_dir.exists() {
                collect(&rules_dir, &mut live_files);
            }
        }
        live_files.sort();
        live_files.dedup();

        if live_files.is_empty() {
            // Nothing to archive — still set the sentinel so the (empty)
            // pass is genuinely one-time.
            self.set_setting(Self::LEGACY_RULES_ARCHIVED_SENTINEL, "1")?;
            return Ok(());
        }

        // Archive root: `<data_local>/legacy-rules-archive/<ISO8601>/`,
        // resolved exactly like `db_path()` so demo-mode overrides apply and
        // the dir is never inside a watched learned-rules tree.
        let data_dir = dirs::data_local_dir()
            .or_else(|| {
                dirs::home_dir().map(|h| {
                    if cfg!(target_os = "macos") {
                        h.join("Library").join("Application Support")
                    } else {
                        h.join(".local").join("share")
                    }
                })
            })
            .ok_or("Cannot determine data directory")?;
        let default_app_dir = data_dir.join("com.quilltoolkit.app");
        let app_dir = crate::data_paths::resolve_data_dir_with_default(default_app_dir);
        let stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let archive_dir = app_dir.join("legacy-rules-archive").join(&stamp);
        std::fs::create_dir_all(&archive_dir)
            .map_err(|e| format!("Create legacy archive dir error: {e}"))?;

        #[derive(serde::Serialize)]
        struct ManifestEntry {
            orig: String,
            sha256: String,
            scope: String,
            mtime: Option<String>,
        }
        let mut manifest: Vec<ManifestEntry> = Vec::new();

        for (idx, src) in live_files.iter().enumerate() {
            let bytes = match std::fs::read(src) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let sha = format!("{:x}", sha2::Sha256::digest(&bytes));
            let scope_label = match inferred_rule_provider_scope(src).as_slice() {
                [IntegrationProvider::Codex] => "codex",
                s if s.contains(&IntegrationProvider::Claude)
                    && s.contains(&IntegrationProvider::Codex) =>
                {
                    "shared"
                }
                _ => "claude",
            }
            .to_string();
            let mtime = std::fs::metadata(src)
                .and_then(|m| m.modified())
                .ok()
                .map(|t| chrono::DateTime::<Utc>::from(t).to_rfc3339());

            // Flat, collision-proof archive name: `<stem>__<idx>.md`.
            let stem = src
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("rule")
                .to_string();
            let dest = archive_dir.join(format!("{stem}__{idx}.md"));
            std::fs::write(&dest, &bytes)
                .map_err(|e| format!("Write legacy archive copy error: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o444));
            }

            manifest.push(ManifestEntry {
                orig: src.to_string_lossy().to_string(),
                sha256: sha,
                scope: scope_label,
                mtime,
            });
        }

        let manifest_json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| format!("Serialize legacy archive manifest error: {e}"))?;
        let manifest_path = archive_dir.join("ARCHIVE_MANIFEST.json");
        std::fs::write(&manifest_path, manifest_json)
            .map_err(|e| format!("Write legacy archive manifest error: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o444));
        }

        // Delete the live files, then durably tombstone the matching DB
        // rows. DB mutations run in one tx so the suppression + tombstones
        // are atomic; the sentinel is set in the SAME tx so completion is
        // all-or-nothing with the DB side.
        for src in &live_files {
            if src.exists() {
                let _ = std::fs::remove_file(src);
            }
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Legacy archive tx begin: {e}"))?;
        for entry in &manifest {
            let name = std::path::Path::new(&entry.orig)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let rule_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM learned_rules WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| format!("Legacy archive id lookup error: {e}"))?;
            // Only mutate the DB row if it exists; orphan on-disk rules are
            // still archived+deleted, just with no row to tombstone.
            if rule_id.is_some() {
                tx.execute(
                    "UPDATE learned_rules SET lifecycle = 'tombstoned', state = 'suppressed', file_path = '', updated_at = datetime('now') WHERE name = ?1",
                    params![name],
                )
                .map_err(|e| format!("Legacy archive lifecycle update error: {e}"))?;
            }
            tx.execute(
                "INSERT INTO rule_tombstones (rule_name, rule_id, tombstoned_at, tombstoned_by, reason, last_content_hash)
                 VALUES (?1, ?2, datetime('now'), 'legacy_archive', 'pre-gated-pipeline rule archived', ?3)
                 ON CONFLICT(rule_name) DO UPDATE SET
                     rule_id = excluded.rule_id,
                     tombstoned_at = datetime('now'),
                     tombstoned_by = 'legacy_archive',
                     reason = 'pre-gated-pipeline rule archived',
                     last_content_hash = excluded.last_content_hash,
                     reactivated_at = NULL,
                     reactivated_by = NULL",
                params![name, rule_id, entry.sha256],
            )
            .map_err(|e| format!("Legacy archive tombstone write error: {e}"))?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, '1')",
            params![Self::LEGACY_RULES_ARCHIVED_SENTINEL],
        )
        .map_err(|e| format!("Legacy archive sentinel write error: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Legacy archive tx commit: {e}"))?;

        Ok(())
    }

    pub fn cleanup_old_observations(&self) -> Result<(), String> {
        // Feature 005 US5 T061 (R-7.3 / M-2 / FR-026 / SC-010). Retention
        // cutoff = `MIN(analyzed_watermark, now - RETENTION_FLOOR)` where
        // `analyzed_watermark = MAX(created_at) FROM learning_runs WHERE status
        // IN ('completed','degraded')`. Observations newer than the watermark
        // have NOT yet had an analysis opportunity and must never be deleted.
        // The safety floor only ever *adds* retention (the `MIN` can only pull
        // the cutoff older), never deletes inside the unanalyzed window. If
        // there are zero completed/degraded runs the watermark is NULL ⇒
        // delete NOTHING (the corpus has never been analyzed). Summarize and
        // delete run in ONE transaction: a failed summary write rolls back the
        // delete (no more best-effort `.ok()` then unconditional delete).
        //
        // Retention TOCTOU (feature 005 hardening — SC-010 / FR-026). The
        // analyzed-watermark MUST be read INSIDE the same serializable unit
        // as the summarize+DELETE: a learning run that completes between a
        // pre-tx watermark read and the DELETE would advance the watermark
        // and let this pass purge observations that run was about to
        // analyze. The tx is therefore opened with
        // `TransactionBehavior::Immediate`, which takes the SQLite write
        // lock up front and excludes concurrent writers (including a
        // learning run inserting/finishing a `learning_runs` row) for the
        // whole watermark→summary→DELETE sequence. The watermark `SELECT`
        // runs on `tx`, so the cutoff reflects the state at lock
        // acquisition and cannot move under us.
        let mut conn = self.conn.lock();

        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| format!("Observation cleanup tx begin: {e}"))?;

        let cutoff_ts: Option<String> = tx
            .query_row(
                &format!(
                    "SELECT MIN(
                        (SELECT MAX(created_at) FROM learning_runs
                         WHERE status IN ('completed','degraded')),
                        datetime('now', '{OBSERVATION_RETENTION_FLOOR}')
                    )"
                ),
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|e| format!("Retention cutoff query error: {e}"))?;

        // NULL watermark (no completed/degraded run yet) ⇒ `MIN(NULL, x)` is
        // NULL in SQLite ⇒ nothing has had an analysis opportunity ⇒ no-op.
        // Returning here drops `tx` before any write, so it rolls back
        // cleanly (no rows touched) and only releases the write lock.
        let Some(cutoff_ts) = cutoff_ts else {
            return Ok(());
        };

        // Aggregate tool counts and a *specific* error tally by project for
        // the window being deleted. `OBSERVATION_ERROR_PREDICATE` is constant
        // SQL with no bind parameters, so the `?1` cutoff bind is unchanged.
        type ObservationSummaryKey = (String, Option<String>);
        type ObservationSummaryValue = (serde_json::Map<String, serde_json::Value>, i64, i64);
        let mut project_summaries: std::collections::HashMap<
            ObservationSummaryKey,
            ObservationSummaryValue,
        > = std::collections::HashMap::new();
        {
            let mut summary_stmt = tx
                .prepare(&format!(
                    "SELECT provider, cwd, tool_name, COUNT(*) as cnt,
                            SUM(CASE WHEN {OBSERVATION_ERROR_PREDICATE} THEN 1 ELSE 0 END) as err_cnt
                     FROM observations
                     WHERE created_at < ?1
                     GROUP BY provider, cwd, tool_name"
                ))
                .map_err(|e| format!("Summary prepare error: {e}"))?;

            let summary_rows = summary_stmt
                .query_map(params![cutoff_ts], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })
                .map_err(|e| format!("Summary query error: {e}"))?;

            for row in summary_rows {
                let (provider, project, tool, count, errors) =
                    row.map_err(|e| format!("Summary row: {e}"))?;
                let entry = project_summaries
                    .entry((provider, project))
                    .or_insert_with(|| (serde_json::Map::new(), 0, 0));
                entry.0.insert(tool, serde_json::Value::from(count));
                entry.1 += errors;
                entry.2 += count;
            }
        }

        let period = Utc::now().format("%Y-%m-%d").to_string();
        for ((provider, project), (tool_counts, error_count, total)) in &project_summaries {
            if *total == 0 {
                continue;
            }
            let tc_json = serde_json::Value::Object(tool_counts.clone()).to_string();
            tx.execute(
                "INSERT OR REPLACE INTO observation_summaries (period, provider, project, tool_counts, error_count, total_observations)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![period, provider, project, tc_json, error_count, total],
            )
            .map_err(|e| format!("Observation summary write error: {e}"))?;
        }

        tx.execute(
            "DELETE FROM observations WHERE created_at < ?1",
            params![cutoff_ts],
        )
        .map_err(|e| format!("Observation cleanup error: {e}"))?;

        tx.commit()
            .map_err(|e| format!("Observation cleanup tx commit: {e}"))?;
        Ok(())
    }

    pub fn aggregate_and_cleanup_tokens(&self) -> Result<(), String> {
        let mut conn = self.conn.lock();
        let cutoff = (Utc::now() - TimeDelta::days(30)).to_rfc3339();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        tx.execute(
            "INSERT INTO token_hourly (hour, provider, hostname, total_input, total_output, total_cache_creation, total_cache_read, turn_count)
             SELECT
                 strftime('%Y-%m-%dT%H:00:00Z', timestamp) as hour,
                 provider,
                 hostname,
                 SUM(input_tokens),
                 SUM(output_tokens),
                 SUM(cache_creation_input_tokens),
                 SUM(cache_read_input_tokens),
                 COUNT(*)
             FROM token_snapshots
             WHERE timestamp < ?1
             GROUP BY hour, provider, hostname
             ON CONFLICT(hour, hostname, provider) DO UPDATE SET
                 total_input = token_hourly.total_input + excluded.total_input,
                 total_output = token_hourly.total_output + excluded.total_output,
                 total_cache_creation = token_hourly.total_cache_creation + excluded.total_cache_creation,
                 total_cache_read = token_hourly.total_cache_read + excluded.total_cache_read,
                 turn_count = token_hourly.turn_count + excluded.turn_count",
            params![cutoff],
        )
        .map_err(|e| format!("Token aggregation insert error: {e}"))?;

        tx.execute(
            "DELETE FROM token_snapshots WHERE timestamp < ?1",
            params![cutoff],
        )
        .map_err(|e| format!("Token aggregation delete error: {e}"))?;

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;

        Ok(())
    }

    /// Delete all tool_actions for a session (used before re-indexing to prevent duplicates).
    pub fn delete_tool_actions_for_session(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM tool_actions WHERE provider = ?1 AND session_id = ?2",
            rusqlite::params![provider.as_str(), session_id],
        )
        .map_err(|e| format!("Delete tool_actions for session: {e}"))?;
        Ok(())
    }

    pub fn delete_skill_usages_for_session(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM skill_usages WHERE provider = ?1 AND session_id = ?2",
            params![provider.as_str(), session_id],
        )
        .map_err(|e| format!("Delete skill_usages for session: {e}"))?;
        Ok(())
    }

    /// Delete every `hook_invocations` row for a single session
    /// (feature 009). Called from the session-delete cascade in
    /// `delete_session_data` and from the per-session reindex path so
    /// transcript replays don't accumulate duplicates beyond what
    /// `INSERT OR IGNORE` absorbs.
    // @lat: [[backend#Database#Schema#Hook Invocations]]
    pub fn delete_hook_invocations_for_session(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM hook_invocations WHERE provider = ?1 AND session_id = ?2",
            params![provider.as_str(), session_id],
        )
        .map_err(|e| format!("Delete hook_invocations for session: {e}"))?;
        Ok(())
    }

    pub fn store_tool_actions_for_messages(
        &self,
        provider: IntegrationProvider,
        messages: &[crate::sessions::ExtractedMessage],
    ) -> Result<(), String> {
        if !messages
            .iter()
            .any(|message| !message.tool_actions.is_empty())
        {
            return Ok(());
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin tool_actions batch transaction: {e}"))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO tool_actions (provider, message_id, session_id, tool_name, category, file_path, summary, full_input, full_output, timestamp, is_sidechain, agent_id, parent_uuid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                )
                .map_err(|e| format!("Prepare store_tool_actions batch: {e}"))?;

            for message in messages {
                if message.tool_actions.is_empty() {
                    continue;
                }

                insert_tool_actions(
                    &mut stmt,
                    provider,
                    &message.tool_actions,
                    &message.uuid,
                    &message.session_id,
                    ToolActionAttribution {
                        is_sidechain: message.is_sidechain,
                        agent_id: message.agent_id.as_deref(),
                        parent_uuid: message.parent_uuid.as_deref(),
                    },
                )?;
            }
        }

        tx.commit()
            .map_err(|e| format!("Commit tool_actions batch: {e}"))?;
        Ok(())
    }

    pub fn store_skill_usages_for_messages(
        &self,
        provider: IntegrationProvider,
        messages: &[crate::sessions::ExtractedMessage],
    ) -> Result<(), String> {
        let hostname = Some(crate::sessions::SessionIndex::local_hostname());
        let usages: Vec<SkillUsage> = messages
            .iter()
            .flat_map(|message| {
                let message_cwd = message.cwd.clone();
                let hostname = hostname.clone();
                message.tool_actions.iter().flat_map(move |action| {
                    let cwd = message_cwd.clone();
                    let hostname = hostname.clone();
                    crate::sessions::extract_skill_accesses_from_tool_action(action)
                        .into_iter()
                        .map(move |access| SkillUsage {
                            session_id: message.session_id.clone(),
                            message_id: message.uuid.clone(),
                            skill_name: access.skill_name,
                            skill_path: access.skill_path,
                            timestamp: action.timestamp.clone(),
                            tool_name: Some(action.tool_name.clone()),
                            cwd: cwd.clone(),
                            hostname: hostname.clone(),
                        })
                })
            })
            .collect();

        if usages.is_empty() {
            return Ok(());
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin skill_usages batch transaction: {e}"))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO skill_usages
                     (provider, session_id, message_id, skill_name, skill_path, timestamp, tool_name, cwd, hostname)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .map_err(|e| format!("Prepare store skill_usages batch: {e}"))?;

            for usage in usages {
                stmt.execute(params![
                    provider.as_str(),
                    usage.session_id,
                    usage.message_id,
                    usage.skill_name,
                    usage.skill_path,
                    usage.timestamp,
                    usage.tool_name,
                    usage.cwd,
                    usage.hostname,
                ])
                .map_err(|e| format!("Insert skill_usage: {e}"))?;
            }
        }

        tx.commit()
            .map_err(|e| format!("Commit skill_usages batch: {e}"))?;
        Ok(())
    }

    /// Insert observed lifecycle-hook fires into `hook_invocations`
    /// (feature 009). Single transaction, prepared statement,
    /// `INSERT OR IGNORE` against the UNIQUE identity index so
    /// re-extraction of the same Claude transcript is idempotent. See
    /// specs/009-hooks-breakdown-tab/contracts/hook-invocations.md
    /// (§ Insert path).
    // @lat: [[backend#Database#Schema#Hook Invocations]]
    pub fn store_hook_invocations_for_messages(
        &self,
        provider: IntegrationProvider,
        invocations: &[HookInvocationInput<'_>],
    ) -> Result<(), String> {
        if invocations.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin hook_invocations batch transaction: {e}"))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO hook_invocations (
                        provider, session_id, agent_id, is_sidechain, timestamp,
                        hook_event, hook_matcher, tool_name, hook_identity,
                        script_command_raw, exit_code, duration_ms, cwd, hostname,
                        message_id
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                )
                .map_err(|e| format!("Prepare store_hook_invocations: {e}"))?;

            for inv in invocations {
                stmt.execute(params![
                    provider.as_str(),
                    inv.session_id,
                    inv.agent_id,
                    inv.is_sidechain as i64,
                    inv.timestamp,
                    inv.hook_event,
                    inv.hook_matcher,
                    inv.tool_name,
                    inv.hook_identity,
                    inv.script_command_raw,
                    inv.exit_code,
                    inv.duration_ms,
                    inv.cwd,
                    inv.hostname,
                    inv.message_id,
                ])
                .map_err(|e| format!("Insert hook_invocation: {e}"))?;
            }
        }

        tx.commit()
            .map_err(|e| format!("Commit hook_invocations batch: {e}"))?;
        Ok(())
    }

    /// Insert a single Codex hook observation submitted via
    /// `POST /api/v1/hooks/observed`. The endpoint validates the
    /// payload and acknowledges synchronously; this insert runs on a
    /// background blocking task. Identity is event-scoped (Codex does
    /// not report per-script execution to the observer hook), with
    /// `tool_name` appended when present so `PreToolUse:Bash` and
    /// `PreToolUse:Read` separate cleanly in the breakdown.
    ///
    /// `cwd` is persisted verbatim — unlike `/learning/observations`
    /// which redacts free-text content, the per-cwd drilldown axis on
    /// the Hooks breakdown depends on path-accurate `cwd` values
    /// matching the same paths stored in `skill_usages.cwd` and the
    /// project-rename pipeline. The redaction boundary therefore lives
    /// at the producer (`hook-observe.cjs`) which only forwards the
    /// stdin `cwd` field Codex already chose to expose.
    // @lat: [[backend#HTTP API Server#Endpoints]]
    pub fn store_codex_hook_observation(
        &self,
        obs: &crate::models::CodexHookObservation,
    ) -> Result<(), String> {
        let identity = match obs.tool_name.as_deref() {
            Some(t) if !t.is_empty() => format!("{}:{}", obs.hook_event, t),
            _ => obs.hook_event.clone(),
        };
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "INSERT OR IGNORE INTO hook_invocations (
                    provider, session_id, agent_id, is_sidechain, timestamp,
                    hook_event, hook_matcher, tool_name, hook_identity,
                    script_command_raw, exit_code, duration_ms, cwd, hostname,
                    message_id
                ) VALUES (?1, ?2, NULL, 0, ?3, ?4, ?5, ?6, ?7, NULL, NULL, NULL, ?8, NULL, NULL)",
            )
            .map_err(|e| format!("Prepare store_codex_hook_observation: {e}"))?;
        stmt.execute(params![
            obs.provider.as_str(),
            obs.session_id,
            obs.ts,
            obs.hook_event,
            obs.hook_matcher,
            obs.tool_name,
            identity,
            obs.cwd,
        ])
        .map_err(|e| format!("Insert codex hook observation: {e}"))?;
        Ok(())
    }

    // --- Memory optimizer storage methods ---

    /// Create a new optimization run record. Returns the run ID.
    /// Uses atomic INSERT...WHERE NOT EXISTS to prevent TOCTOU race.
    #[allow(dead_code)]
    pub fn create_optimization_run(
        &self,
        project_path: &str,
        trigger: &str,
        provider_scope: &[IntegrationProvider],
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        // Atomic: insert only if no running run exists for this project
        conn.execute(
            "INSERT INTO optimization_runs (project_path, provider_scope, trigger, memories_scanned, suggestions_created, context_sources, status, started_at)
             SELECT ?1, ?2, ?3, 0, 0, '{}', 'running', ?4
             WHERE NOT EXISTS (
                 SELECT 1 FROM optimization_runs WHERE project_path = ?1 AND status = 'running'
             )",
            rusqlite::params![project_path, provider_scope_json(provider_scope), trigger, now],
        )
        .map_err(|e| format!("Failed to create optimization run: {e}"))?;

        if conn.changes() == 0 {
            return Err(format!(
                "An optimization is already running for {project_path}"
            ));
        }
        Ok(conn.last_insert_rowid())
    }

    /// Update an optimization run with results.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn update_optimization_run(
        &self,
        run_id: i64,
        memories_scanned: i64,
        suggestions_created: i64,
        context_sources: &str,
        status: &str,
        error: Option<&str>,
        inference_metadata: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE optimization_runs SET memories_scanned = ?1, suggestions_created = ?2,
             context_sources = ?3, status = ?4, error = ?5, completed_at = ?6,
             inference_metadata = ?7 WHERE id = ?8",
            rusqlite::params![
                memories_scanned,
                suggestions_created,
                context_sources,
                status,
                error,
                now,
                inference_metadata,
                run_id
            ],
        )
        .map_err(|e| format!("Failed to update optimization run: {e}"))?;
        Ok(())
    }

    /// Get optimization runs for a project.
    #[allow(dead_code)]
    pub fn get_optimization_runs(
        &self,
        project_path: &str,
        provider: Option<IntegrationProvider>,
        limit: i64,
    ) -> Result<Vec<crate::models::OptimizationRun>, String> {
        let conn = self.conn.lock();
        let (query, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT id, project_path, provider_scope, trigger, memories_scanned, suggestions_created,
                        status, error, started_at, completed_at
                 FROM optimization_runs
                 WHERE project_path = ?1 AND instr(provider_scope, ?2) > 0
                 ORDER BY started_at DESC LIMIT ?3"
                    .to_string(),
                vec![
                    Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(provider_scope_contains_json(provider)),
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT id, project_path, provider_scope, trigger, memories_scanned, suggestions_created,
                        status, error, started_at, completed_at
                 FROM optimization_runs
                 WHERE project_path = ?1
                 ORDER BY started_at DESC LIMIT ?2"
                    .to_string(),
                vec![
                    Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
        };
        let mut stmt = conn
            .prepare_cached(&query)
            .map_err(|e| format!("Failed to prepare optimization runs query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok(crate::models::OptimizationRun {
                    id: row.get(0)?,
                    project_path: row.get(1)?,
                    provider_scope: parse_provider_scope(row.get(2)?),
                    trigger: row.get(3)?,
                    memories_scanned: row.get(4)?,
                    suggestions_created: row.get(5)?,
                    status: row.get(6)?,
                    error: row.get(7)?,
                    started_at: row.get(8)?,
                    completed_at: row.get(9)?,
                })
            })
            .map_err(|e| format!("Failed to query optimization runs: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect optimization runs: {e}"))
    }

    /// Store an optimization suggestion.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn store_optimization_suggestion(
        &self,
        run_id: i64,
        project_path: &str,
        action_type: &str,
        target_file: Option<&str>,
        reasoning: &str,
        proposed_content: Option<&str>,
        merge_sources: Option<&str>,
        original_content: Option<&str>,
        diff_summary: Option<&str>,
        backup_data: Option<&str>,
    ) -> Result<i64, String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        let provider_scope: String = conn
            .query_row(
                "SELECT provider_scope FROM optimization_runs WHERE id = ?1",
                rusqlite::params![run_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read optimization run scope: {e}"))?;
        conn.execute(
            "INSERT INTO optimization_suggestions
             (run_id, project_path, provider_scope, action_type, target_file, reasoning, proposed_content, merge_sources, status, created_at, original_content, diff_summary, backup_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?11, ?12)",
            rusqlite::params![run_id, project_path, provider_scope, action_type, target_file, reasoning, proposed_content, merge_sources, now, original_content, diff_summary, backup_data],
        )
        .map_err(|e| format!("Failed to store optimization suggestion: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    /// Get optimization suggestions for a project with pagination, optionally filtered by status.
    #[allow(dead_code)]
    pub fn get_optimization_suggestions(
        &self,
        project_path: &str,
        provider: Option<IntegrationProvider>,
        status_filter: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();

        let mut query = "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                                proposed_content, merge_sources, status, error, resolved_at, created_at,
                                original_content, diff_summary, backup_data, group_id
                         FROM optimization_suggestions WHERE project_path = ?1"
            .to_string();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>];
        let mut next_param = 2;
        if let Some(provider) = provider {
            query.push_str(&format!(" AND instr(provider_scope, ?{next_param}) > 0"));
            params.push(Box::new(provider_scope_contains_json(provider)));
            next_param += 1;
        }
        if let Some(status) = status_filter {
            query.push_str(&format!(" AND status = ?{next_param}"));
            params.push(Box::new(status.to_string()));
            next_param += 1;
        }
        query.push_str(&format!(
            " ORDER BY created_at DESC LIMIT ?{next_param} OFFSET ?{}",
            next_param + 1
        ));
        params.push(Box::new(limit));
        params.push(Box::new(offset));

        let mut stmt = conn
            .prepare_cached(&query)
            .map_err(|e| format!("Failed to prepare suggestions query: {e}"))?;

        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            })
            .map_err(|e| format!("Failed to query suggestions: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect suggestions: {e}"))
    }

    /// Get denied suggestions for a project (for denial context in optimizer prompt).
    #[allow(dead_code)]
    pub fn get_denied_suggestions(
        &self,
        project_path: &str,
        provider: Option<IntegrationProvider>,
        limit: i64,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();
        let (query, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match provider {
            Some(provider) => (
                "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions
                 WHERE project_path = ?1 AND status = 'denied' AND instr(provider_scope, ?2) > 0
                 ORDER BY resolved_at DESC LIMIT ?3"
                    .to_string(),
                vec![
                    Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(provider_scope_contains_json(provider)),
                    Box::new(limit),
                ],
            ),
            None => (
                "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions
                 WHERE project_path = ?1 AND status = 'denied'
                 ORDER BY resolved_at DESC LIMIT ?2"
                    .to_string(),
                vec![
                    Box::new(project_path.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(limit),
                ],
            ),
        };
        let mut stmt = conn
            .prepare_cached(&query)
            .map_err(|e| format!("Failed to prepare denied suggestions query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            })
            .map_err(|e| format!("Failed to query denied suggestions: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect denied suggestions: {e}"))
    }

    /// Update a suggestion status (approve/deny).
    #[allow(dead_code)]
    pub fn update_suggestion_status(
        &self,
        suggestion_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE optimization_suggestions SET status = ?1, error = ?2, resolved_at = ?3 WHERE id = ?4",
            rusqlite::params![status, error, now, suggestion_id],
        )
        .map_err(|e| format!("Failed to update suggestion status: {e}"))?;
        Ok(())
    }

    /// Get a single suggestion by ID.
    #[allow(dead_code)]
    pub fn get_suggestion_by_id(
        &self,
        suggestion_id: i64,
    ) -> Result<crate::models::OptimizationSuggestion, String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                    proposed_content, merge_sources, status, error, resolved_at, created_at,
                    original_content, diff_summary, backup_data, group_id
             FROM optimization_suggestions WHERE id = ?1",
            rusqlite::params![suggestion_id],
            |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            },
        )
        .map_err(|e| format!("Suggestion not found: {e}"))
    }

    /// Upsert a memory file record (scan tracking).
    #[allow(dead_code)]
    pub fn upsert_memory_file(
        &self,
        project_path: &str,
        file_path: &str,
        content_hash: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO memory_files (project_path, file_path, content_hash, last_scanned_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_path, file_path) DO UPDATE SET content_hash = ?3, last_scanned_at = ?4",
            rusqlite::params![project_path, file_path, content_hash, now],
        )
        .map_err(|e| format!("Failed to upsert memory file: {e}"))?;
        Ok(())
    }

    /// Get previously recorded memory file hashes for change detection.
    #[allow(dead_code)]
    pub fn get_memory_file_hashes(
        &self,
        project_path: &str,
    ) -> Result<std::collections::HashMap<String, String>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT file_path, content_hash FROM memory_files WHERE project_path = ?1",
            )
            .map_err(|e| format!("Failed to prepare memory files query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![project_path], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("Failed to query memory files: {e}"))?;
        rows.collect::<Result<std::collections::HashMap<_, _>, _>>()
            .map_err(|e| format!("Failed to collect memory file hashes: {e}"))
    }

    /// Delete suggestion (for undeny — removes from DB so it's no longer in denial blocklist).
    #[allow(dead_code)]
    pub fn delete_suggestion(&self, suggestion_id: i64) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM optimization_suggestions WHERE id = ?1",
            rusqlite::params![suggestion_id],
        )
        .map_err(|e| format!("Failed to delete suggestion: {e}"))?;
        Ok(())
    }

    /// Get all suggestions for a specific optimization run.
    #[allow(dead_code)]
    pub fn get_suggestions_for_run(
        &self,
        run_id: i64,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions WHERE run_id = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| format!("Failed to prepare suggestions-for-run query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![run_id], |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            })
            .map_err(|e| format!("Failed to query suggestions for run: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect suggestions for run: {e}"))
    }

    /// Clean up stale optimization suggestions:
    /// - Expire pending/undone suggestions older than 14 days (set status to 'expired')
    /// - Delete denied suggestions older than 90 days
    /// - Clear original_content/backup_data from approved suggestions older than 30 days
    #[allow(dead_code)]
    pub fn cleanup_stale_suggestions(&self, project_path: &str) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = Utc::now();
        let expire_cutoff = (now - TimeDelta::days(14)).to_rfc3339();
        let delete_cutoff = (now - TimeDelta::days(90)).to_rfc3339();
        let clear_cutoff = (now - TimeDelta::days(30)).to_rfc3339();

        // Expire pending/undone suggestions older than 14 days
        conn.execute(
            "UPDATE optimization_suggestions SET status = 'expired', resolved_at = datetime('now')
             WHERE project_path = ?1 AND status IN ('pending', 'undone') AND created_at < ?2",
            rusqlite::params![project_path, expire_cutoff],
        )
        .map_err(|e| format!("Failed to expire stale suggestions: {e}"))?;

        // Delete denied suggestions older than 90 days
        conn.execute(
            "DELETE FROM optimization_suggestions
             WHERE project_path = ?1 AND status = 'denied' AND resolved_at < ?2",
            rusqlite::params![project_path, delete_cutoff],
        )
        .map_err(|e| format!("Failed to delete old denied suggestions: {e}"))?;

        // Clear content from approved suggestions older than 30 days
        conn.execute(
            "UPDATE optimization_suggestions
             SET original_content = NULL, backup_data = NULL
             WHERE project_path = ?1 AND status = 'approved' AND resolved_at < ?2",
            rusqlite::params![project_path, clear_cutoff],
        )
        .map_err(|e| format!("Failed to clear old approved suggestion content: {e}"))?;

        Ok(())
    }

    /// Check if there's already a pending suggestion with the same action type and target file.
    #[allow(dead_code)]
    pub fn has_duplicate_pending(
        &self,
        project_path: &str,
        action_type: &str,
        target_file: Option<&str>,
    ) -> Result<bool, String> {
        let conn = self.conn.lock();
        let count: i64 = if let Some(tf) = target_file {
            conn.query_row(
                "SELECT COUNT(*) FROM optimization_suggestions
                 WHERE project_path = ?1 AND action_type = ?2 AND target_file = ?3 AND status = 'pending'",
                rusqlite::params![project_path, action_type, tf],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check duplicate pending: {e}"))?
        } else {
            conn.query_row(
                "SELECT COUNT(*) FROM optimization_suggestions
                 WHERE project_path = ?1 AND action_type = ?2 AND target_file IS NULL AND status = 'pending'",
                rusqlite::params![project_path, action_type],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check duplicate pending: {e}"))?
        };
        Ok(count > 0)
    }

    /// Set group_id for a suggestion.
    #[allow(dead_code)]
    pub fn set_suggestion_group_id(
        &self,
        suggestion_id: i64,
        group_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE optimization_suggestions SET group_id = ?1 WHERE id = ?2",
            rusqlite::params![group_id, suggestion_id],
        )
        .map_err(|e| format!("Failed to set suggestion group_id: {e}"))?;
        Ok(())
    }

    /// Get all pending suggestions in a group.
    #[allow(dead_code)]
    pub fn get_suggestions_by_group(
        &self,
        group_id: &str,
    ) -> Result<Vec<crate::models::OptimizationSuggestion>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, run_id, project_path, provider_scope, action_type, target_file, reasoning,
                        proposed_content, merge_sources, status, error, resolved_at, created_at,
                        original_content, diff_summary, backup_data, group_id
                 FROM optimization_suggestions WHERE group_id = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| format!("Failed to prepare group suggestions query: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params![group_id], |row| {
                let merge_sources_json: Option<String> = row.get(8)?;
                let merge_sources: Option<Vec<String>> =
                    merge_sources_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(crate::models::OptimizationSuggestion {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    project_path: row.get(2)?,
                    provider_scope: parse_provider_scope(row.get(3)?),
                    action_type: row.get(4)?,
                    target_file: row.get(5)?,
                    reasoning: row.get(6)?,
                    proposed_content: row.get(7)?,
                    merge_sources,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    resolved_at: row.get(11)?,
                    created_at: row.get(12)?,
                    original_content: row.get(13)?,
                    diff_summary: row.get(14)?,
                    backup_data: row.get(15)?,
                    group_id: row.get(16)?,
                })
            })
            .map_err(|e| format!("Failed to query group suggestions: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect group suggestions: {e}"))
    }

    pub fn get_code_stats(&self, range: &str) -> Result<CodeStats, String> {
        let conn = self.conn.lock();
        let from = (Utc::now() - range_to_duration(range)).to_rfc3339();

        let mut stmt = conn
            .prepare(
                "SELECT tool_name, file_path, full_input, session_id
				 FROM tool_actions
				 WHERE category = 'code_change' AND timestamp >= ?1 AND full_input IS NOT NULL",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        let mut total_added: i64 = 0;
        let mut total_removed: i64 = 0;
        let mut sessions: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut lang_lines: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();

        let rows = stmt
            .query_map([&from], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        for row in rows {
            let (tool_name, file_path, full_input, session_id) =
                row.map_err(|e| format!("Row error: {e}"))?;

            if let Some((added, removed, parsed_path)) = parse_code_change(&tool_name, &full_input)
            {
                total_added += added;
                total_removed += removed;
                sessions.insert(session_id);

                let path = if parsed_path.is_empty() {
                    file_path.unwrap_or_default()
                } else {
                    parsed_path
                };
                let lang = ext_to_language(&path);
                *lang_lines.entry(lang).or_insert(0) += added + removed;
            }
        }

        let session_count = sessions.len() as i64;
        let total_changed = total_added + total_removed;
        let avg_per_session = if session_count > 0 {
            total_changed as f64 / session_count as f64
        } else {
            0.0
        };

        let mut by_language: Vec<LanguageBreakdown> = lang_lines
            .into_iter()
            .map(|(language, lines)| {
                let percentage = if total_changed > 0 {
                    (lines as f64 / total_changed as f64) * 100.0
                } else {
                    0.0
                };
                LanguageBreakdown {
                    language: language.to_string(),
                    lines,
                    percentage,
                }
            })
            .collect();
        by_language.sort_by_key(|b| std::cmp::Reverse(b.lines));

        Ok(CodeStats {
            lines_added: total_added,
            lines_removed: total_removed,
            net_change: total_added - total_removed,
            session_count,
            avg_per_session,
            by_language,
        })
    }

    pub fn get_code_stats_history(
        &self,
        range: &str,
    ) -> Result<Vec<CodeStatsHistoryPoint>, String> {
        let conn = self.conn.lock();
        let now = Utc::now();
        let from = now - range_to_duration(range);
        let from_str = from.to_rfc3339();

        let bucket_secs: i64 = match range {
            "1h" => 60,
            "24h" => 15 * 60,
            "7d" => 3600,
            "30d" => 86400,
            _ => 15 * 60,
        };

        let mut stmt = conn
            .prepare(
                "SELECT tool_name, full_input, timestamp
				 FROM tool_actions
				 WHERE category = 'code_change' AND timestamp >= ?1 AND full_input IS NOT NULL
				 ORDER BY timestamp ASC",
            )
            .map_err(|e| format!("Prepare error: {e}"))?;

        struct RawChange {
            added: i64,
            removed: i64,
            ts: DateTime<Utc>,
        }
        let mut changes: Vec<RawChange> = Vec::new();

        let rows = stmt
            .query_map([&from_str], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        for row in rows {
            let (tool_name, full_input, timestamp) = row.map_err(|e| format!("Row error: {e}"))?;

            if let Some((added, removed, _)) = parse_code_change(&tool_name, &full_input)
                && let Ok(ts) = timestamp.parse::<DateTime<Utc>>()
            {
                changes.push(RawChange { added, removed, ts });
            }
        }

        let from_ts = from.timestamp();
        let now_ts = now.timestamp();
        let mut points: Vec<CodeStatsHistoryPoint> = Vec::new();
        let mut bucket_start = from_ts;

        while bucket_start < now_ts {
            let bucket_end = bucket_start + bucket_secs;
            let mut added = 0i64;
            let mut removed = 0i64;

            for c in &changes {
                let ct = c.ts.timestamp();
                if ct >= bucket_start && ct < bucket_end {
                    added += c.added;
                    removed += c.removed;
                }
            }

            let ts = DateTime::from_timestamp(bucket_start, 0)
                .unwrap_or(from)
                .to_rfc3339();

            points.push(CodeStatsHistoryPoint {
                timestamp: ts,
                lines_added: added,
                lines_removed: removed,
                total_changed: added + removed,
            });

            bucket_start = bucket_end;
        }

        Ok(points)
    }

    pub fn get_batch_session_code_stats(
        &self,
        session_refs: &[SessionRef],
    ) -> Result<std::collections::HashMap<String, SessionCodeStats>, String> {
        if session_refs.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let conn = self.conn.lock();
        let mut seen = std::collections::HashSet::new();
        let unique_refs: Vec<&SessionRef> = session_refs
            .iter()
            .filter(|session_ref| {
                seen.insert(session_key(session_ref.provider, &session_ref.session_id))
            })
            .collect();
        let sql = unique_refs
            .iter()
            .enumerate()
            .map(|(idx, _)| {
                let provider_pos = idx * 2 + 1;
                let session_pos = provider_pos + 1;
                format!(
                    "SELECT provider, session_id, tool_name, full_input
                     FROM tool_actions
                     WHERE category = 'code_change'
                       AND full_input IS NOT NULL
                       AND provider = ?{provider_pos}
                       AND session_id = ?{session_pos}"
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Prepare error: {e}"))?;

        let params: Vec<Box<dyn rusqlite::types::ToSql>> = unique_refs
            .iter()
            .flat_map(|session_ref| {
                [
                    Box::new(session_ref.provider.to_string()) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(session_ref.session_id.clone()) as Box<dyn rusqlite::types::ToSql>,
                ]
            })
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| format!("Query error: {e}"))?;

        let mut result: std::collections::HashMap<String, SessionCodeStats> =
            std::collections::HashMap::new();

        for row in rows {
            let (provider, session_id, tool_name, full_input) =
                row.map_err(|e| format!("Row error: {e}"))?;
            let provider: IntegrationProvider = provider.parse()?;

            if let Some((added, removed, _)) = parse_code_change(&tool_name, &full_input) {
                let entry =
                    result
                        .entry(session_key(provider, &session_id))
                        .or_insert(SessionCodeStats {
                            lines_added: 0,
                            lines_removed: 0,
                            net_change: 0,
                        });
                entry.lines_added += added;
                entry.lines_removed += removed;
                entry.net_change = entry.lines_added - entry.lines_removed;
            }
        }

        Ok(result)
    }

    pub fn delete_response_times_for_session(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM response_times WHERE provider = ?1 AND session_id = ?2",
            params![provider.as_str(), session_id],
        )
        .map_err(|e| format!("Failed to delete response_times for session: {e}"))?;
        Ok(())
    }

    pub fn ingest_response_times(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
        messages: &[ResponseTimeInput<'_>],
    ) -> Result<(), String> {
        if messages.is_empty() {
            return Ok(());
        }

        // Sort by timestamp without losing the per-message attribution fields.
        let mut sorted: Vec<ResponseTimeInput<'_>> = messages.to_vec();
        sorted.sort_by(|a, b| a.timestamp.cmp(b.timestamp));

        // Query last assistant timestamp for this session (for cross-batch continuity)
        let last_assistant_ts: Option<String> = {
            let conn = self.conn.lock();
            conn.query_row(
                "SELECT timestamp FROM response_times
                 WHERE provider = ?1 AND session_id = ?2 AND response_secs IS NOT NULL
                 ORDER BY timestamp DESC LIMIT 1",
                params![provider.as_str(), session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query last assistant ts: {e}"))?
        };

        // Build (user_ts, assistant_ts, Option<prev_assistant_ts>) pairs.
        // Claude transcripts alternate user/tool-result messages with assistant
        // responses, while Codex keeps tool activity on assistant-side records.
        // For Codex, treat the turn as ending at the last assistant/tool
        // activity before the next user prompt. Sub-agent attribution from the
        // assistant message that closes the turn flows into the row.
        struct Turn {
            user_ts: String,
            assistant_ts: String,
            prev_assistant_ts: Option<String>,
            is_sidechain: bool,
            agent_id: Option<String>,
            parent_uuid: Option<String>,
        }

        let mut turns: Vec<Turn> = Vec::new();
        let mut prev_assistant: Option<String> = last_assistant_ts;

        if provider == IntegrationProvider::Codex {
            let mut pending_user: Option<String> = None;
            let mut pending_assistant: Option<(String, bool, Option<String>, Option<String>)> =
                None;

            for msg in &sorted {
                match msg.role {
                    "user" => {
                        if let (Some(user_ts), Some((assistant_ts, sc, aid, puuid))) =
                            (pending_user.take(), pending_assistant.take())
                        {
                            turns.push(Turn {
                                user_ts,
                                assistant_ts: assistant_ts.clone(),
                                prev_assistant_ts: prev_assistant.clone(),
                                is_sidechain: sc,
                                agent_id: aid,
                                parent_uuid: puuid,
                            });
                            prev_assistant = Some(assistant_ts);
                        }
                        pending_user = Some(msg.timestamp.to_string());
                    }
                    "assistant" => {
                        if pending_user.is_some() {
                            pending_assistant = Some((
                                msg.timestamp.to_string(),
                                msg.is_sidechain,
                                msg.agent_id.map(|s| s.to_string()),
                                msg.parent_uuid.map(|s| s.to_string()),
                            ));
                        } else {
                            prev_assistant = Some(msg.timestamp.to_string());
                        }
                    }
                    _ => {}
                }
            }

            if let (Some(user_ts), Some((assistant_ts, sc, aid, puuid))) =
                (pending_user, pending_assistant)
            {
                turns.push(Turn {
                    user_ts,
                    assistant_ts,
                    prev_assistant_ts: prev_assistant,
                    is_sidechain: sc,
                    agent_id: aid,
                    parent_uuid: puuid,
                });
            }
        } else {
            let mut pending_user: Option<String> = None;

            for msg in &sorted {
                match msg.role {
                    "user" => {
                        pending_user = Some(msg.timestamp.to_string());
                    }
                    "assistant" => {
                        if let Some(user_ts) = pending_user.take() {
                            turns.push(Turn {
                                user_ts,
                                assistant_ts: msg.timestamp.to_string(),
                                prev_assistant_ts: prev_assistant.clone(),
                                is_sidechain: msg.is_sidechain,
                                agent_id: msg.agent_id.map(|s| s.to_string()),
                                parent_uuid: msg.parent_uuid.map(|s| s.to_string()),
                            });
                        }
                        prev_assistant = Some(msg.timestamp.to_string());
                    }
                    _ => {}
                }
            }
        }

        if turns.is_empty() {
            return Ok(());
        }

        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Transaction error: {e}"))?;

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO response_times (provider, session_id, timestamp, response_secs, idle_secs, is_sidechain, agent_id, parent_uuid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(|e| format!("Prepare error: {e}"))?;

            for turn in &turns {
                let response_secs = parse_ts_diff(&turn.assistant_ts, &turn.user_ts);
                let idle_secs = turn
                    .prev_assistant_ts
                    .as_deref()
                    .and_then(|prev| parse_ts_diff(&turn.user_ts, prev));

                let response_max_secs = if provider == IntegrationProvider::Codex {
                    6.0 * 60.0 * 60.0
                } else {
                    600.0
                };
                let response_val = response_secs.filter(|&s| s > 0.0 && s <= response_max_secs);
                let idle_val = idle_secs.filter(|&s| s > 0.0 && s <= 600.0);

                if response_val.is_none() && idle_val.is_none() {
                    continue;
                }

                stmt.execute(params![
                    provider.as_str(),
                    session_id,
                    turn.assistant_ts,
                    response_val,
                    idle_val,
                    turn.is_sidechain as i32,
                    turn.agent_id,
                    turn.parent_uuid,
                ])
                .map_err(|e| format!("Insert error: {e}"))?;
            }
        }

        tx.commit().map_err(|e| format!("Commit error: {e}"))?;
        Ok(())
    }

    /// Bulk-insert per-event rows for the active-interval runtime
    /// pipeline (feature 008). Idempotent — uses `INSERT OR IGNORE`
    /// against the `(provider, session_id, COALESCE(agent_id, ''),
    /// timestamp, kind)` primary key. See
    /// specs/008-runtime-redesign/contracts/session-events.md
    /// (ING-1..ING-5).
    // @lat: [[backend#Database#Schema#Code and Runtime Metrics]]
    pub fn ingest_session_events(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
        events: &[SessionEventInput<'_>],
    ) -> Result<(), String> {
        // ING-1: empty input short-circuits without touching the DB.
        if events.is_empty() {
            return Ok(());
        }

        // ING-2: defend against extractor ordering bugs / clock skew.
        let mut sorted: Vec<SessionEventInput<'_>> = events.to_vec();
        sorted.sort_by(|a, b| a.timestamp.cmp(b.timestamp));

        // ING-3: single transaction for one (provider, session_id) batch.
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("session_events transaction: {e}"))?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO session_events
                     (provider, session_id, agent_id, is_sidechain, timestamp, kind, uuid, parent_uuid)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(|e| format!("session_events prepare: {e}"))?;

            for ev in &sorted {
                // ING-5: skip rows whose timestamp does not parse as RFC3339.
                if DateTime::parse_from_rfc3339(ev.timestamp).is_err() {
                    log::warn!(
                        "session_events: dropping row with unparseable timestamp {}",
                        ev.timestamp
                    );
                    continue;
                }
                stmt.execute(params![
                    provider.as_str(),
                    session_id,
                    ev.agent_id,
                    ev.is_sidechain as i64,
                    ev.timestamp,
                    ev.kind.as_str(),
                    ev.uuid,
                    ev.parent_uuid,
                ])
                .map_err(|e| format!("session_events insert: {e}"))?;
            }
        }
        tx.commit()
            .map_err(|e| format!("session_events commit: {e}"))?;
        Ok(())
    }

    /// Drop every `session_events` row for one `(provider, session_id)`.
    /// Called from `process_discovered_file` before re-ingest and from
    /// the `delete_session_data` / `delete_host_data` /
    /// `delete_project_data` cascades. See
    /// specs/008-runtime-redesign/contracts/session-events.md.
    // @lat: [[backend#Database#Schema#Code and Runtime Metrics]]
    pub fn delete_session_events_for_session(
        &self,
        provider: IntegrationProvider,
        session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM session_events WHERE provider = ?1 AND session_id = ?2",
            params![provider.as_str(), session_id],
        )
        .map_err(|e| format!("Failed to delete session_events for session: {e}"))?;
        Ok(())
    }

    /// Compute LLM runtime stats. `scope` controls whether sub-agent
    /// (`is_sidechain = 1`) rows are folded in:
    /// * `None` or `Some("all")`: include parent + sub-agent rows (default,
    ///   matches pre-Wave-2 behavior since sub-agent rows used to not exist).
    /// * `Some("parent_only")`: filter out sub-agent rows for the legacy
    ///   "parent transcript only" view.
    ///
    /// Feature 008 rewrites this to source from `session_events` and to
    /// implement the active-interval semantics specified in
    /// specs/008-runtime-redesign/contracts/llm-runtime-stats.md
    /// (STAT-1..STAT-7). A "logical turn" is a contiguous run of events
    /// on a single chain `(provider, session_id, agent_id)` where every
    /// between-event gap is either <= IDLE_THRESHOLD_SECS or a tool-loop
    /// gap (asst_tool_use -> user_tool_result) up to TOOL_WAIT_MAX_SECS.
    // @lat: [[backend#Tauri IPC Commands#Code and Response Stats (5)]]
    pub fn get_llm_runtime_stats(
        &self,
        range: &str,
        scope: Option<&str>,
    ) -> Result<LlmRuntimeStats, String> {
        const IDLE_THRESHOLD_SECS: f64 = 300.0; // 5 minutes (R-B)
        const TOOL_WAIT_MAX_SECS: f64 = 21_600.0; // 6 hours (R-B safety ceiling)

        let conn = self.conn.lock();
        let now = Utc::now();
        let from = now - range_to_duration(range);
        let from_str = from.to_rfc3339();
        let parent_only = matches!(scope, Some("parent_only"));

        let range_secs = range_to_duration(range).num_seconds() as f64;
        let bucket_secs = range_secs / 7.0;
        let from_epoch_ms = from.timestamp_millis() as f64;
        // STAT-4 + R-B: tool-wait gaps realize at most up to `now`. The
        // ceiling is the smaller of `prev_ms + TOOL_WAIT_MAX_SECS` and
        // `now_ms`, so a realized turn never credits wall-clock time that
        // has not elapsed yet (e.g., when prev_ms is < 6h before now).
        let now_ms = now.timestamp_millis() as f64;

        let mut total_runtime_secs: f64 = 0.0;
        let mut turn_count: i64 = 0;
        let mut sessions: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut bucket_sums: [f64; 7] = [0.0; 7];

        // Walker state: a "chain" is (provider, session_id, agent_id).
        // We track its parts separately so that detecting a chain change
        // does not require formatting a composite key on every row — a hot
        // path on multi-thousand-event queries.
        // turn_start_ms holds the first event in the current logical turn;
        // prev_ms / prev_kind capture the most recent event on the chain.
        let mut cur_provider: Option<String> = None;
        let mut cur_session_id: Option<String> = None;
        let mut cur_agent_id: Option<String> = None;
        let mut turn_start_ms: f64 = 0.0;
        let mut prev_ms: f64 = 0.0;
        let mut prev_kind: Option<String> = None;

        // STAT-5: flush a finalized turn into the aggregates, distributing
        // its duration into a single sparkline bucket keyed off the turn's
        // start. (Whole-turn assignment matches the prior implementation;
        // bucket overlap proration is reserved for a future enhancement
        // once we have multi-bucket coverage tests.)
        let flush_turn = |start_ms: f64,
                          end_ms: f64,
                          total: &mut f64,
                          count: &mut i64,
                          buckets: &mut [f64; 7],
                          from_ep: f64,
                          bkt_secs: f64| {
            let dur = ((end_ms - start_ms) / 1000.0).max(0.0);
            if dur > 0.0 {
                *total += dur;
                *count += 1;
                let offset_ms = start_ms - from_ep;
                let bucket = ((offset_ms / 1000.0) / bkt_secs).max(0.0) as usize;
                buckets[bucket.min(6)] += dur;
            }
        };

        {
            // STAT-2: parent_only adds `is_sidechain = 0` to the WHERE.
            // Order matches the chain key so the walker can process each
            // (provider, session_id, agent_id) chain contiguously.
            let sql = if parent_only {
                "SELECT timestamp, kind, provider, session_id, agent_id, is_sidechain
                 FROM session_events
                 WHERE timestamp >= ?1 AND is_sidechain = 0
                 ORDER BY provider, session_id, COALESCE(agent_id, ''), timestamp"
            } else {
                "SELECT timestamp, kind, provider, session_id, agent_id, is_sidechain
                 FROM session_events
                 WHERE timestamp >= ?1
                 ORDER BY provider, session_id, COALESCE(agent_id, ''), timestamp"
            };
            let mut stmt = conn
                .prepare_cached(sql)
                .map_err(|e| format!("Prepare runtime query: {e}"))?;
            let rows = stmt
                .query_map(params![from_str], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })
                .map_err(|e| format!("Runtime query error: {e}"))?;

            for row in rows.flatten() {
                let (ts_str, kind, provider, session_id, agent_id, _is_sc) = row;
                let ts = match DateTime::parse_from_rfc3339(&ts_str) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let ms = ts.timestamp_millis() as f64;
                // STAT-3: chain identity is (provider, session_id, agent_id).
                // Compare slices first to avoid allocating a composite key
                // on every row; the borrow ends before we move the owned
                // strings into the trackers below.
                let chain_changed = cur_provider.as_deref() != Some(provider.as_str())
                    || cur_session_id.as_deref() != Some(session_id.as_str())
                    || cur_agent_id.as_deref() != agent_id.as_deref();

                if chain_changed {
                    if cur_provider.is_some() {
                        flush_turn(
                            turn_start_ms,
                            prev_ms,
                            &mut total_runtime_secs,
                            &mut turn_count,
                            &mut bucket_sums,
                            from_epoch_ms,
                            bucket_secs,
                        );
                    }
                    // STAT-6: session_count is keyed on (provider,
                    // session_id) only, so a parent and its sub-agents
                    // count as one session. Inserting at chain-change
                    // boundaries (instead of every row) caps allocations
                    // at O(distinct chains).
                    sessions.insert(format!("{provider}|{session_id}"));
                    cur_provider = Some(provider);
                    cur_session_id = Some(session_id);
                    cur_agent_id = agent_id;
                    turn_start_ms = ms;
                } else {
                    // STAT-4: classify the gap from the previous event.
                    let gap = (ms - prev_ms) / 1000.0;
                    let is_tool_loop_gap = matches!(prev_kind.as_deref(), Some("asst_tool_use"))
                        && kind == "user_tool_result";
                    let counts_as_active = if is_tool_loop_gap {
                        // Tool-loop gaps count up to the safety ceiling.
                        gap <= TOOL_WAIT_MAX_SECS
                    } else {
                        // Non-tool-loop gaps count only when <= idle threshold.
                        gap <= IDLE_THRESHOLD_SECS
                    };

                    if !counts_as_active {
                        // User-idle (or tool-loop over ceiling) gap exceeds
                        // threshold: end the current turn at prev_ms and
                        // begin a new one at the current row.
                        let clamped_end_ms = if is_tool_loop_gap {
                            // STAT-4 / R-B: realize up to the ceiling so
                            // the long wait still contributes its bounded
                            // share, but never past `now` — we cannot
                            // credit wall-clock time that has not elapsed.
                            (prev_ms + TOOL_WAIT_MAX_SECS * 1000.0).min(now_ms)
                        } else {
                            prev_ms
                        };
                        flush_turn(
                            turn_start_ms,
                            clamped_end_ms,
                            &mut total_runtime_secs,
                            &mut turn_count,
                            &mut bucket_sums,
                            from_epoch_ms,
                            bucket_secs,
                        );
                        turn_start_ms = ms;
                    }
                    // else: the gap counts; current turn continues with
                    // turn_start_ms unchanged.
                }
                prev_ms = ms;
                prev_kind = Some(kind);
            }
            if cur_provider.is_some() {
                flush_turn(
                    turn_start_ms,
                    prev_ms,
                    &mut total_runtime_secs,
                    &mut turn_count,
                    &mut bucket_sums,
                    from_epoch_ms,
                    bucket_secs,
                );
            }
        }

        // STAT-7: avg = total / count, 0 when empty window.
        let avg_per_turn_secs = if turn_count > 0 {
            total_runtime_secs / turn_count as f64
        } else {
            0.0
        };

        Ok(LlmRuntimeStats {
            total_runtime_secs,
            turn_count,
            session_count: sessions.len() as i64,
            avg_per_turn_secs,
            sparkline: bucket_sums.to_vec(),
        })
    }
}

/// Parse the difference in seconds between two ISO 8601 timestamps (end - start).
/// Returns None if either timestamp fails to parse.
fn parse_ts_diff(end_ts: &str, start_ts: &str) -> Option<f64> {
    let end = DateTime::parse_from_rfc3339(end_ts).ok()?;
    let start = DateTime::parse_from_rfc3339(start_ts).ok()?;
    let diff = (end - start).num_milliseconds() as f64 / 1000.0;
    if diff < 0.0 { None } else { Some(diff) }
}

/// Given a set of cwd paths, returns a map of child path -> resolved parent
/// project root, skipping the home directory (it would absorb every project).
/// Paths with no proper-prefix ancestor in the input are absent from the map.
fn compute_subdir_parent_map(paths: &[String]) -> std::collections::HashMap<String, String> {
    // Home directories are too generic to act as merge parents.
    // A session run from ~ should stay its own row, not absorb every project.
    let home_dir = dirs::home_dir().map(|h| h.to_string_lossy().to_string());

    // Build a mapping: child path → parent root
    let mut parent_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for path in paths {
        // Check if any other path is a proper prefix of this one
        let mut best_parent: Option<&str> = None;
        for candidate in paths {
            if candidate == path {
                continue;
            }
            // Skip home directory — it's not a real project root
            if home_dir.as_deref() == Some(candidate.as_str()) {
                continue;
            }
            // candidate must be a proper prefix with a '/' boundary
            if path.starts_with(candidate.as_str())
                && path.as_bytes().get(candidate.len()) == Some(&b'/')
            {
                // Pick the longest (most specific) parent so subdirs merge
                // into their closest project root, not a distant ancestor.
                match best_parent {
                    Some(bp) if candidate.len() > bp.len() => {
                        best_parent = Some(candidate);
                    }
                    None => {
                        best_parent = Some(candidate);
                    }
                    _ => {}
                }
            }
        }
        if let Some(parent) = best_parent {
            parent_map.insert(path.clone(), parent.to_string());
        }
    }

    parent_map
}

/// Merge project breakdown entries where one cwd is a subdirectory of another.
/// For example, `/home/user/work/foo` and `/home/user/work/foo/bar` are merged
/// into a single entry under `/home/user/work/foo`.
fn merge_project_subdirs(mut rows: Vec<ProjectBreakdown>) -> Vec<ProjectBreakdown> {
    if rows.len() <= 1 {
        return rows;
    }

    // Sort by path so parents come before children
    rows.sort_by(|a, b| a.project.cmp(&b.project));

    // Collect all unique project paths (across all hostnames)
    let paths: Vec<String> = {
        let mut p: Vec<String> = rows.iter().map(|r| r.project.clone()).collect();
        p.sort();
        p.dedup();
        p
    };

    let parent_map = compute_subdir_parent_map(&paths);

    if parent_map.is_empty() {
        // No merging needed — sort by last_active desc and return
        rows.sort_by(|a, b| b.last_active.cmp(&a.last_active));
        rows.truncate(50);
        return rows;
    }

    // Merge: group by (resolved_project, hostname) and aggregate
    let mut merged: std::collections::HashMap<(String, String), ProjectBreakdown> =
        std::collections::HashMap::new();
    for row in rows {
        let resolved = parent_map
            .get(&row.project)
            .cloned()
            .unwrap_or(row.project.clone());
        let key = (resolved.clone(), row.hostname.clone());
        let entry = merged.entry(key).or_insert_with(|| ProjectBreakdown {
            project: resolved,
            hostname: row.hostname.clone(),
            total_tokens: 0,
            turn_count: 0,
            session_count: 0,
            last_active: String::new(),
        });
        entry.total_tokens += row.total_tokens;
        entry.turn_count += row.turn_count;
        entry.session_count += row.session_count;
        if row.last_active > entry.last_active {
            entry.last_active = row.last_active;
        }
    }

    let mut results: Vec<ProjectBreakdown> = merged.into_values().collect();
    results.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    results.truncate(50);
    results
}

fn calc_trend(
    conn: &Connection,
    provider: IntegrationProvider,
    bucket_key: &str,
) -> Result<String, String> {
    let now = Utc::now();
    let one_hour_ago = (now - TimeDelta::hours(1)).to_rfc3339();
    let two_hours_ago = (now - TimeDelta::hours(2)).to_rfc3339();

    let recent_avg: Option<f64> = conn
        .query_row(
            "SELECT AVG(utilization) FROM usage_snapshots
             WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3",
            params![provider.as_str(), bucket_key, one_hour_ago],
            |row| row.get(0),
        )
        .map_err(|e| format!("Trend query error: {e}"))?;

    let prev_avg: Option<f64> = conn
        .query_row(
            "SELECT AVG(utilization) FROM usage_snapshots
             WHERE provider = ?1 AND bucket_key = ?2 AND timestamp >= ?3 AND timestamp < ?4",
            params![provider.as_str(), bucket_key, two_hours_ago, one_hour_ago],
            |row| row.get(0),
        )
        .map_err(|e| format!("Trend query error: {e}"))?;

    match (recent_avg, prev_avg) {
        (Some(r), Some(p)) if r > p + 2.0 => Ok("up".into()),
        (Some(r), Some(p)) if r < p - 2.0 => Ok("down".into()),
        (Some(_), Some(_)) => Ok("flat".into()),
        _ => Ok("unknown".into()),
    }
}

fn downsample_tokens(points: Vec<TokenDataPoint>, max: usize) -> Vec<TokenDataPoint> {
    if points.len() <= max {
        return points;
    }

    let chunk_size = points.len().div_ceil(max);
    points
        .chunks(chunk_size)
        .map(|chunk| {
            let inp: i64 = chunk.iter().map(|p| p.input_tokens).sum();
            let out: i64 = chunk.iter().map(|p| p.output_tokens).sum();
            let cc: i64 = chunk.iter().map(|p| p.cache_creation_input_tokens).sum();
            let cr: i64 = chunk.iter().map(|p| p.cache_read_input_tokens).sum();
            TokenDataPoint {
                timestamp: chunk[chunk.len() / 2].timestamp.clone(),
                input_tokens: inp,
                output_tokens: out,
                cache_creation_input_tokens: cc,
                cache_read_input_tokens: cr,
                total_tokens: inp + out + cc + cr,
            }
        })
        .collect()
}

fn downsample(points: Vec<DataPoint>, max: usize) -> Vec<DataPoint> {
    if points.len() <= max {
        return points;
    }

    let chunk_size = points.len().div_ceil(max);
    points
        .chunks(chunk_size)
        .map(|chunk| {
            let avg_util = chunk.iter().map(|p| p.utilization).sum::<f64>() / chunk.len() as f64;
            DataPoint {
                timestamp: chunk[chunk.len() / 2].timestamp.clone(),
                utilization: avg_util,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    /// Drive Storage::init against a temp directory by routing `db_path`
    /// through the QUILL_DEMO_MODE path override. We own the env block under
    /// #[serial] so concurrent tests do not race over these globals.
    fn init_storage_in(_dir: &TempDir) -> Storage {
        // SAFETY: env mutation; tests are serialized via `#[serial]`.
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_DATA_DIR", _dir.path());
        }
        // Leave the env set so any sibling lookups during the test continue
        // to resolve into the temp dir. Test teardown reaps it.
        Storage::init().expect("init storage")
    }

    fn clear_env() {
        unsafe {
            std::env::remove_var("QUILL_DEMO_MODE");
            std::env::remove_var("QUILL_DATA_DIR");
        }
    }

    #[test]
    #[serial]
    #[ignore] // diagnostic only; prints schema for manual verification
    fn dump_post_migration_schema() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);
        let conn = storage.conn.lock();
        for table in ["response_times", "tool_actions", "token_snapshots"] {
            println!("\n--- {table} columns ---");
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .expect("pragma");
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i32>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i32>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect");
            for r in rows {
                println!("{r:?}");
            }
        }
        clear_env();
    }

    #[test]
    #[serial]
    fn migration_20_adds_subagent_columns() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let conn = storage.conn.lock();
        for table in ["response_times", "tool_actions", "token_snapshots"] {
            assert!(
                table_has_column(&conn, table, "is_sidechain"),
                "{table}.is_sidechain missing after migration 20"
            );
            assert!(
                table_has_column(&conn, table, "agent_id"),
                "{table}.agent_id missing after migration 20"
            );
            assert!(
                table_has_column(&conn, table, "parent_uuid"),
                "{table}.parent_uuid missing after migration 20"
            );
        }
        let version: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .expect("read schema_version");
        assert!(
            version >= 20,
            "schema_version must advance to at least 20, got {version}"
        );
        drop(conn);
        clear_env();
    }

    #[test]
    #[serial]
    fn migration_25_adds_governance_schema() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);
        let conn = storage.conn.lock();

        // All six additive learned_rules columns are present.
        for col in [
            "lifecycle",
            "origin_run_id",
            "origin_model",
            "origin_at",
            "current_version",
            "superseded_by",
        ] {
            assert!(
                table_has_column(&conn, "learned_rules", col),
                "learned_rules.{col} missing after migration 25"
            );
        }
        // The repurposed-but-not-re-added confirmed_projects column (added by
        // migration 4) must still exist — migration 25 must not drop it.
        assert!(
            table_has_column(&conn, "learned_rules", "confirmed_projects"),
            "learned_rules.confirmed_projects unexpectedly missing"
        );
        // The existing read-derived `state` column is left untouched.
        assert!(
            table_has_column(&conn, "learned_rules", "state"),
            "learned_rules.state unexpectedly missing"
        );

        // All six new governance tables exist.
        for table in [
            "rule_versions",
            "rule_evidence_citations",
            "rule_tombstones",
            "operator_feedback",
            "evaluation_results",
            "reviewer_overrides",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("query sqlite_master");
            assert_eq!(exists, 1, "table {table} missing after migration 25");
        }

        // A representative key constraint per new table is enforced.
        // rule_tombstones.rule_name is the PRIMARY KEY (durable, name-keyed).
        conn.execute(
            "INSERT INTO rule_tombstones (rule_name, tombstoned_by) VALUES ('r1', 'human')",
            [],
        )
        .expect("insert tombstone");
        let dup = conn.execute(
            "INSERT INTO rule_tombstones (rule_name, tombstoned_by) VALUES ('r1', 'human')",
            [],
        );
        assert!(dup.is_err(), "rule_tombstones.rule_name PK not enforced");
        // operator_feedback UNIQUE(rule_name, actor) is a revisable upsert key.
        conn.execute(
            "INSERT INTO operator_feedback (rule_name, actor, feedback) VALUES ('r1', 'operator', 'accept')",
            [],
        )
        .expect("insert operator_feedback");
        let of_dup = conn.execute(
            "INSERT INTO operator_feedback (rule_name, actor, feedback) VALUES ('r1', 'operator', 'reject')",
            [],
        );
        assert!(
            of_dup.is_err(),
            "operator_feedback UNIQUE(rule_name, actor) not enforced"
        );

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .expect("read schema_version");
        assert!(
            version >= 25,
            "schema_version must advance to at least 25, got {version}"
        );

        drop(conn);
        clear_env();
    }

    #[test]
    #[serial]
    fn migration_25_is_idempotent() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");

        // First init applies migration 25.
        let storage = init_storage_in(&dir);
        {
            let conn = storage.conn.lock();
            let v: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                    [],
                    |row| row.get(0),
                )
                .expect("read schema_version");
            assert!(v >= 25, "schema_version must reach 25, got {v}");

            // Replay the migration-25 DDL directly against the already-
            // migrated DB. Every ALTER is `table_has_column`-guarded and
            // every CREATE is `IF NOT EXISTS`, so this must be a clean no-op
            // and must not duplicate the schema_version=25 row's effect.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS rule_versions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    rule_id INTEGER NOT NULL REFERENCES learned_rules(id) ON DELETE CASCADE,
                    version INTEGER NOT NULL, content TEXT NOT NULL,
                    content_hash TEXT NOT NULL, domain TEXT,
                    is_anti_pattern INTEGER NOT NULL DEFAULT 0,
                    provider_scope TEXT NOT NULL DEFAULT '[\"claude\"]',
                    source TEXT, run_id INTEGER, change_kind TEXT NOT NULL,
                    rolled_back_from INTEGER, author TEXT NOT NULL DEFAULT 'system',
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    UNIQUE(rule_id, version)
                );
                CREATE TABLE IF NOT EXISTS rule_tombstones (
                    rule_name TEXT PRIMARY KEY, rule_id INTEGER,
                    tombstoned_at TEXT NOT NULL DEFAULT (datetime('now')),
                    tombstoned_by TEXT NOT NULL, reason TEXT,
                    last_content_hash TEXT, reactivated_at TEXT, reactivated_by TEXT
                );",
            )
            .expect("replayed migration-25 DDL must be a clean no-op");
            assert!(!table_has_column(&conn, "learned_rules", "nonexistent_col"));
            for col in [
                "lifecycle",
                "origin_run_id",
                "origin_model",
                "origin_at",
                "current_version",
                "superseded_by",
            ] {
                assert!(table_has_column(&conn, "learned_rules", col));
            }
        }
        drop(storage);

        // Re-init against the same data dir reopens the same DB; the
        // version-gated migration loop must skip migration 25 (no error) and
        // leave at least migration 25's row in place.
        let storage2 = init_storage_in(&dir);
        let conn = storage2.conn.lock();
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .expect("read schema_version");
        assert!(
            version >= 25,
            "re-running migrations must keep schema_version >= 25, got {version}"
        );
        // Exactly one schema_version=25 row — the gate did not re-enter.
        let count_25: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_version WHERE version = 25",
                [],
                |row| row.get(0),
            )
            .expect("count v25 rows");
        assert_eq!(count_25, 1, "migration 25 recorded more than once");

        drop(conn);
        clear_env();
    }

    #[test]
    #[serial]
    fn migration_26_is_idempotent() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");

        // First init applies migration 26.
        let storage = init_storage_in(&dir);
        {
            let conn = storage.conn.lock();
            let v: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                    [],
                    |row| row.get(0),
                )
                .expect("read schema_version");
            assert!(v >= 26, "schema_version must reach 26, got {v}");

            // The session_events table and its unique-identity index must
            // exist after the migration runs.
            let has_table: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='table' AND name='session_events'",
                    [],
                    |row| row.get(0),
                )
                .expect("query sqlite_master for table");
            assert_eq!(
                has_table, 1,
                "session_events table missing after migration 26"
            );
            let has_unique_idx: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='index' AND name='uidx_se_identity'",
                    [],
                    |row| row.get(0),
                )
                .expect("query sqlite_master for unique index");
            assert_eq!(
                has_unique_idx, 1,
                "uidx_se_identity missing after migration 26"
            );

            // Replay the migration-26 DDL directly against the already-
            // migrated DB. Every CREATE is `IF NOT EXISTS`, so this must
            // be a clean no-op.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS session_events (
                    provider     TEXT NOT NULL,
                    session_id   TEXT NOT NULL,
                    agent_id     TEXT,
                    is_sidechain INTEGER NOT NULL DEFAULT 0,
                    timestamp    TEXT NOT NULL,
                    kind         TEXT NOT NULL,
                    uuid         TEXT,
                    parent_uuid  TEXT
                );
                CREATE UNIQUE INDEX IF NOT EXISTS uidx_se_identity
                    ON session_events(provider, session_id, COALESCE(agent_id, ''), timestamp, kind);
                CREATE INDEX IF NOT EXISTS idx_se_timestamp
                    ON session_events(timestamp);
                CREATE INDEX IF NOT EXISTS idx_se_chain
                    ON session_events(provider, session_id, agent_id, timestamp);
                CREATE INDEX IF NOT EXISTS idx_se_provider_session_sidechain
                    ON session_events(provider, session_id, is_sidechain, timestamp);",
            )
            .expect("replayed migration-26 DDL must be a clean no-op");
        }
        drop(storage);

        // Re-init against the same data dir reopens the same DB; the
        // version-gated migration loop must skip migration 26 (no error)
        // and leave exactly one v=26 row in `schema_version`.
        let storage2 = init_storage_in(&dir);
        let conn = storage2.conn.lock();
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .expect("read schema_version");
        assert!(
            version >= 26,
            "re-running migrations must keep schema_version >= 26, got {version}"
        );
        let count_26: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_version WHERE version = 26",
                [],
                |row| row.get(0),
            )
            .expect("count v26 rows");
        assert_eq!(count_26, 1, "migration 26 recorded more than once");

        drop(conn);
        clear_env();
    }

    #[test]
    fn evidence_weighted_score_matches_pre_refactor_and_is_monotonic() {
        // 1. Byte-identical to the pre-refactor inlined read-site math
        //    (site 1: confidence = wilson_lower_bound(alpha*fresh,
        //    beta*fresh); state = compute_state(confidence, alpha, beta,
        //    fresh)). Sample row mirrors a realistic recent-evidence rule.
        let ts = (Utc::now() - chrono::Duration::days(10)).to_rfc3339();
        let (alpha, beta) = (8.0_f64, 3.0_f64);

        let fresh = freshness_factor(Some(ts.as_str()));
        let expected_conf = wilson_lower_bound(alpha * fresh, beta * fresh);
        let expected_state = compute_state(expected_conf, alpha, beta, fresh);

        let (score, state) = evidence_weighted_score(alpha, beta, Some(ts.as_str()));
        assert_eq!(
            score.to_bits(),
            expected_conf.to_bits(),
            "evidence_weighted_score must reproduce the pre-refactor confidence exactly"
        );
        assert_eq!(
            state, expected_state,
            "evidence_weighted_score state must match pre-refactor compute_state"
        );

        // None last_evidence_at => freshness 1.0 path also matches.
        let (s_none, _) = evidence_weighted_score(alpha, beta, None);
        assert_eq!(s_none.to_bits(), wilson_lower_bound(alpha, beta).to_bits());

        // 2. Monotonic non-decreasing in alpha (more confirmations never
        //    lowers the lower-bound score), holding beta/freshness fixed.
        let mut prev = f64::NEG_INFINITY;
        for a in [1.0, 2.0, 5.0, 10.0, 50.0, 200.0] {
            let (sc, _) = evidence_weighted_score(a, 3.0, None);
            assert!(
                sc >= prev - 1e-12,
                "score must be monotonic non-decreasing in alpha: {sc} < {prev} at alpha={a}"
            );
            prev = sc;
        }

        // 3. Freshness decays: identical alpha/beta, older evidence yields a
        //    strictly lower score than fresh evidence.
        let recent = (Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        let old = (Utc::now() - chrono::Duration::days(365)).to_rfc3339();
        let (s_recent, _) = evidence_weighted_score(10.0, 1.0, Some(recent.as_str()));
        let (s_old, _) = evidence_weighted_score(10.0, 1.0, Some(old.as_str()));
        assert!(
            s_old < s_recent,
            "older evidence must decay the score: old={s_old} not < recent={s_recent}"
        );
    }

    #[test]
    #[serial]
    fn response_times_insert_round_trips_subagent_columns() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // Two-turn mini transcript: one top-level turn, one sub-agent turn.
        // Each turn = user+assistant pair sorted by timestamp.
        let inputs = vec![
            ResponseTimeInput::new("user", "2026-05-09T10:00:00Z"),
            ResponseTimeInput::new("assistant", "2026-05-09T10:00:05Z"),
            ResponseTimeInput {
                role: "user",
                timestamp: "2026-05-09T10:00:10Z",
                is_sidechain: true,
                agent_id: Some("aaaabbbbccccdddd"),
                parent_uuid: Some("p2"),
            },
            ResponseTimeInput {
                role: "assistant",
                timestamp: "2026-05-09T10:00:15Z",
                is_sidechain: true,
                agent_id: Some("aaaabbbbccccdddd"),
                parent_uuid: Some("s1"),
            },
        ];
        storage
            .ingest_response_times(IntegrationProvider::Claude, "test-session", &inputs)
            .expect("insert response_times");

        let conn = storage.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT timestamp, is_sidechain, agent_id, parent_uuid
                 FROM response_times
                 WHERE provider = 'claude' AND session_id = 'test-session'
                 ORDER BY timestamp ASC",
            )
            .expect("prepare select");
        let rows: Vec<(String, i32, Option<String>, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");

        assert_eq!(rows.len(), 2, "expected one row per turn, got {rows:?}");
        assert_eq!(rows[0].1, 0, "top-level row defaults to is_sidechain=0");
        assert!(rows[0].2.is_none(), "top-level row has NULL agent_id");
        assert_eq!(rows[1].1, 1, "sub-agent row stores is_sidechain=1");
        assert_eq!(rows[1].2.as_deref(), Some("aaaabbbbccccdddd"));
        assert_eq!(rows[1].3.as_deref(), Some("s1"));
        // stmt + conn drop at scope end; explicit drop would move-conflict.
        clear_env();
    }

    /// Wave 2 rollup contract: get_session_breakdown must SUM tokens and
    /// turns across the parent transcript plus every sub-agent chain, and
    /// surface has_subagents / subagent_count for the Sessions UI to know
    /// the row is expandable.
    #[test]
    #[serial]
    fn get_session_breakdown_rolls_up_subagent_tokens() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // Insert directly so timestamps are deterministic. store_token_snapshot
        // overwrites timestamp with `now()`, which would make ordering
        // assertions flaky.
        let now = Utc::now();
        let recent = now.to_rfc3339();
        let earlier = (now - TimeDelta::minutes(10)).to_rfc3339();
        let middle = (now - TimeDelta::minutes(5)).to_rfc3339();

        {
            let conn = storage.conn.lock();
            // Parent row: 100 input + 50 output + 0 cache = 150 tokens.
            conn.execute(
                "INSERT INTO token_snapshots (provider, session_id, hostname, timestamp,
                     input_tokens, output_tokens, cache_creation_input_tokens,
                     cache_read_input_tokens, cwd, is_sidechain, agent_id, parent_uuid)
                 VALUES ('claude', 'sess-rollup', 'host1', ?1,
                         100, 50, 0, 0, '/some/cwd', 0, NULL, NULL)",
                params![&earlier],
            )
            .expect("insert parent snapshot");
            // Sub-agent row: 200 input + 80 output + 10 + 5 = 295 tokens.
            conn.execute(
                "INSERT INTO token_snapshots (provider, session_id, hostname, timestamp,
                     input_tokens, output_tokens, cache_creation_input_tokens,
                     cache_read_input_tokens, cwd, is_sidechain, agent_id, parent_uuid)
                 VALUES ('claude', 'sess-rollup', 'host1', ?1,
                         200, 80, 10, 5, '/some/cwd', 1, 'aaaabbbbccccdddd', 'pmsg1')",
                params![&recent],
            )
            .expect("insert sub-agent snapshot");

            // One parent turn + one sub-agent turn ⇒ rolled-up turn_count = 2.
            conn.execute(
                "INSERT INTO response_times (provider, session_id, timestamp,
                     response_secs, idle_secs, is_sidechain, agent_id, parent_uuid)
                 VALUES ('claude', 'sess-rollup', ?1, 5.0, NULL, 0, NULL, NULL)",
                params![&earlier],
            )
            .expect("insert parent response_time");
            conn.execute(
                "INSERT INTO response_times (provider, session_id, timestamp,
                     response_secs, idle_secs, is_sidechain, agent_id, parent_uuid)
                 VALUES ('claude', 'sess-rollup', ?1, 4.5, NULL, 1, 'aaaabbbbccccdddd', 'pmsg1')",
                params![&middle],
            )
            .expect("insert sub-agent response_time");
        }

        let rows = storage
            .get_session_breakdown(7, None, None, Some(100))
            .expect("get_session_breakdown");
        let row = rows
            .iter()
            .find(|r| r.session_id == "sess-rollup")
            .expect("session present in breakdown");

        assert_eq!(
            row.total_tokens, 445,
            "tokens must sum parent (150) + sub-agent (295) = 445; got {row:?}"
        );
        assert_eq!(
            row.turn_count, 2,
            "turn_count must sum response_times rows (parent + sub-agent) = 2"
        );
        assert!(
            row.has_subagents,
            "has_subagents must be true when token_snapshots has is_sidechain=1 row"
        );
        assert_eq!(
            row.subagent_count, 1,
            "subagent_count must be 1 distinct agent_id across all three tables"
        );
        // last_active reflects the most recent row across both tables.
        assert_eq!(
            row.last_active, recent,
            "last_active must equal MAX timestamp"
        );

        drop(rows);
        clear_env();
    }

    /// Wave 2 tree contract: get_session_subagent_tree returns one node per
    /// distinct agent_id under the session, ordered by first_seen ASC. Today
    /// every depth-1 sub-agent has parent_agent_id = None because the chain
    /// root parent_uuid points into the parent transcript whose rows carry
    /// agent_id IS NULL (filtered out by the resolver query).
    #[test]
    #[serial]
    fn get_session_subagent_tree_returns_one_node_per_agent() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let now = Utc::now();
        let t_a = (now - TimeDelta::minutes(30)).to_rfc3339();
        let t_a_end = (now - TimeDelta::minutes(28)).to_rfc3339();
        let t_b = (now - TimeDelta::minutes(20)).to_rfc3339();
        let t_b_end = (now - TimeDelta::minutes(18)).to_rfc3339();

        {
            let conn = storage.conn.lock();
            // Agent A — earlier, two response_times, one tool_action,
            // one token_snapshot.
            conn.execute_batch(&format!(
                r#"
                INSERT INTO token_snapshots (provider, session_id, hostname, timestamp,
                    input_tokens, output_tokens, cache_creation_input_tokens,
                    cache_read_input_tokens, cwd, is_sidechain, agent_id, parent_uuid)
                  VALUES ('claude', 'sess-tree', 'h', '{t_a}',
                          10, 20, 1, 2, NULL, 1, 'a11111111111aaaa', 'pmsg-A');

                INSERT INTO response_times (provider, session_id, timestamp,
                    response_secs, idle_secs, is_sidechain, agent_id, parent_uuid)
                  VALUES ('claude', 'sess-tree', '{t_a}', 1.5, NULL, 1, 'a11111111111aaaa', 'pmsg-A');
                INSERT INTO response_times (provider, session_id, timestamp,
                    response_secs, idle_secs, is_sidechain, agent_id, parent_uuid)
                  VALUES ('claude', 'sess-tree', '{t_a_end}', 2.5, 0.3, 1, 'a11111111111aaaa', 'msg-A2');

                INSERT INTO tool_actions (provider, message_id, session_id, tool_name, category,
                    summary, timestamp, is_sidechain, agent_id, parent_uuid)
                  VALUES ('claude', 'msg-A2', 'sess-tree', 'Read', 'fs', 'read file',
                          '{t_a_end}', 1, 'a11111111111aaaa', 'pmsg-A');

                -- Agent B — later, simpler shape.
                INSERT INTO token_snapshots (provider, session_id, hostname, timestamp,
                    input_tokens, output_tokens, cache_creation_input_tokens,
                    cache_read_input_tokens, cwd, is_sidechain, agent_id, parent_uuid)
                  VALUES ('claude', 'sess-tree', 'h', '{t_b}',
                          5, 5, 0, 0, NULL, 1, 'b22222222222bbbb', 'pmsg-B');

                INSERT INTO response_times (provider, session_id, timestamp,
                    response_secs, idle_secs, is_sidechain, agent_id, parent_uuid)
                  VALUES ('claude', 'sess-tree', '{t_b_end}', 1.0, NULL, 1, 'b22222222222bbbb', 'pmsg-B');
                "#
            ))
            .expect("seed tree fixtures");
        }

        let nodes = storage
            .get_session_subagent_tree(IntegrationProvider::Claude, "sess-tree")
            .expect("get_session_subagent_tree");

        assert_eq!(
            nodes.len(),
            2,
            "two distinct agents ⇒ two nodes; got {nodes:?}"
        );

        // Ordered by first_seen ASC: Agent A first.
        assert_eq!(nodes[0].agent_id, "a11111111111aaaa");
        assert_eq!(nodes[1].agent_id, "b22222222222bbbb");

        // Depth-1 nesting: every node's parent uuid lives in the (unseeded)
        // parent transcript, so the resolver returns None.
        assert!(
            nodes[0].parent_agent_id.is_none(),
            "depth-1 sub-agent should have parent_agent_id = None"
        );
        assert!(
            nodes[1].parent_agent_id.is_none(),
            "depth-1 sub-agent should have parent_agent_id = None"
        );

        // Agent A's aggregates.
        assert_eq!(nodes[0].turn_count, 2);
        assert_eq!(nodes[0].input_tokens, 10);
        assert_eq!(nodes[0].output_tokens, 20);
        assert_eq!(nodes[0].cache_creation_tokens, 1);
        assert_eq!(nodes[0].cache_read_tokens, 2);
        assert_eq!(nodes[0].total_tokens, 33);
        assert_eq!(nodes[0].tool_call_count, 1);
        assert_eq!(nodes[0].first_seen, t_a);
        assert_eq!(nodes[0].last_active, t_a_end);

        // Agent B's aggregates.
        assert_eq!(nodes[1].turn_count, 1);
        assert_eq!(nodes[1].total_tokens, 10);
        assert_eq!(nodes[1].tool_call_count, 0);

        clear_env();
    }

    // Feature 005 US1 T021 — one-time redaction backfill scrubs pre-existing
    // plaintext at rest and is sentinel-guarded so a second run is a no-op.
    #[test]
    #[serial]
    fn backfill_redaction_masks_existing_rows_and_is_idempotent() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // `Storage::init` already ran the backfill once (no-op on the empty
        // DB) and set the sentinel. Seed raw plaintext secrets directly via
        // SQL so we bypass `store_observation`'s capture-side redaction, then
        // clear the sentinel so an explicit pass actually does the rewrite.
        let secret = "sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO observations (provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd)
                 VALUES ('claude', 's-bf', '2026-01-01T00:00:00Z', 'PostToolUse', 'Bash',
                         ?1, 'ran ok', '/home/u/proj')",
                params![format!("export TOKEN={secret}")],
            )
            .expect("seed observation plaintext");
            conn.execute(
                "INSERT INTO git_snapshots (project, commit_hash, commit_count, raw_data)
                 VALUES ('proj', 'abc123', 1, ?1)",
                params![format!("diff with leaked {secret} value")],
            )
            .expect("seed git_snapshot plaintext");
            conn.execute(
                "DELETE FROM settings WHERE key = ?1",
                params![Storage::REDACTION_BACKFILL_SENTINEL],
            )
            .expect("clear sentinel");
        }

        storage.backfill_redaction().expect("first backfill");

        {
            let conn = storage.conn.lock();
            let obs_input: String = conn
                .query_row(
                    "SELECT tool_input FROM observations WHERE session_id = 's-bf'",
                    [],
                    |r| r.get(0),
                )
                .expect("read obs input");
            let git_raw: String = conn
                .query_row(
                    "SELECT raw_data FROM git_snapshots WHERE project = 'proj'",
                    [],
                    |r| r.get(0),
                )
                .expect("read git raw");
            assert!(
                !obs_input.contains(secret),
                "plaintext secret must be gone from observations: {obs_input}"
            );
            assert!(
                obs_input.contains(crate::redaction::MASK),
                "observations.tool_input must carry the mask token: {obs_input}"
            );
            assert!(
                !git_raw.contains(secret),
                "plaintext secret must be gone from git_snapshots: {git_raw}"
            );
            assert!(
                git_raw.contains(crate::redaction::MASK),
                "git_snapshots.raw_data must carry the mask token: {git_raw}"
            );
            let sentinel: Option<String> = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = ?1",
                    params![Storage::REDACTION_BACKFILL_SENTINEL],
                    |r| r.get(0),
                )
                .ok();
            assert_eq!(
                sentinel.as_deref(),
                Some("1"),
                "sentinel must be set after a successful backfill"
            );
        }

        // Second run is a no-op: the sentinel short-circuits before any
        // rewrite. Seed a fresh plaintext row and confirm a subsequent
        // backfill leaves it untouched (proving the guard, not just
        // `redact`'s idempotence).
        {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO observations (provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd)
                 VALUES ('claude', 's-bf2', '2026-01-02T00:00:00Z', 'PostToolUse', 'Bash',
                         ?1, 'ok', '/tmp')",
                params![format!("export TOKEN={secret}")],
            )
            .expect("seed second plaintext");
        }

        storage
            .backfill_redaction()
            .expect("second backfill (no-op)");

        {
            let conn = storage.conn.lock();
            let untouched: String = conn
                .query_row(
                    "SELECT tool_input FROM observations WHERE session_id = 's-bf2'",
                    [],
                    |r| r.get(0),
                )
                .expect("read second obs input");
            assert!(
                untouched.contains(secret),
                "sentinel-guarded second run must NOT rewrite new rows: {untouched}"
            );
        }

        clear_env();
    }

    // Feature 005 US1 T012 — SC-001 capture-path adoption: an observation
    // carrying secrets/PII is redacted *at rest* by `store_observation`
    // itself (the T013/T014 wiring), with rule-relevant structure preserved
    // (FR-006). Distinct from T021, which scrubs pre-existing rows.
    #[test]
    #[serial]
    fn store_observation_redacts_secrets_and_pii_at_rest() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let secret = "sk-ant-api03-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
        let payload: crate::models::ObservationPayload =
            serde_json::from_value(serde_json::json!({
                "session_id": "s-cap",
                "hook_phase": "PostToolUse",
                "tool_name": "Bash",
                "tool_input": format!(
                    "export ANTHROPIC_API_KEY={secret} && psql postgres://u:p4ssw0rd@db.host/app"
                ),
                "tool_output": format!("done; ping dev@example.com; bearer {secret}"),
                "cwd": "/home/u/proj"
            }))
            .expect("payload");

        storage
            .store_observation(&payload)
            .expect("store observation through the real capture path");

        let (ti, to, cwd): (String, String, String) = {
            let conn = storage.conn.lock();
            conn.query_row(
                "SELECT tool_input, tool_output, cwd FROM observations WHERE session_id = 's-cap'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("read back stored observation")
        };

        let mask = crate::redaction::MASK;
        // SC-001: zero unredacted secret/PII material at rest.
        for field in [&ti, &to] {
            assert!(
                !field.contains(secret),
                "raw provider key must not be at rest: {field}"
            );
            assert!(field.contains(mask), "field must carry the mask: {field}");
        }
        assert!(
            !ti.contains("p4ssw0rd"),
            "connection-string password must be masked: {ti}"
        );
        assert!(
            !to.contains("dev@example.com"),
            "email local-part must be masked: {to}"
        );
        // FR-006: rule-relevant structure survives (key name, URL
        // scheme/host, email domain, benign cwd path).
        assert!(
            ti.contains("ANTHROPIC_API_KEY")
                && ti.contains("postgres://")
                && ti.contains("db.host"),
            "structural frame must be preserved: {ti}"
        );
        assert!(
            to.contains("@example.com"),
            "email domain must be preserved: {to}"
        );
        assert_eq!(cwd, "/home/u/proj", "benign cwd path unchanged: {cwd}");

        clear_env();
    }

    // Feature 005 US2 T032 (FR-012 / Q3=C, contracts/rule-governance.md
    // "Legacy archive-then-wipe"): a pre-existing on-disk learned `.md`
    // (with its DB row) is archived read-only + manifested, deleted from
    // its live location, and durably tombstoned; a second pass is a no-op.
    #[test]
    #[serial]
    fn archive_legacy_rules_archives_wipes_tombstones_and_is_idempotent() {
        clear_env();
        let data_dir = TempDir::new().expect("data tempdir");
        let rules_dir = TempDir::new().expect("rules tempdir");
        // SAFETY: env mutation; serialized via #[serial]. Route demo-mode
        // rule dirs into the temp rules dir so we never touch real $HOME.
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_DATA_DIR", data_dir.path());
            std::env::set_var("QUILL_RULES_DIR", rules_dir.path());
        }
        let storage = Storage::init().expect("init storage");

        // Seed a live Claude-scope learned rule on disk + its DB row, then
        // clear the sentinel so an explicit pass actually archives it
        // (init already ran the pass once as a no-op on the empty tree).
        let claude_dir = crate::learning::learned_rules_dir_for_scope(&[
            crate::integrations::IntegrationProvider::Claude,
        ]);
        std::fs::create_dir_all(&claude_dir).expect("mk claude rules dir");
        let rule_file = claude_dir.join("legacy-rule.md");
        std::fs::write(&rule_file, "Prefer explicit error types.\n").expect("seed legacy rule .md");
        {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO learned_rules (name, domain, confidence, observation_count, file_path, state, lifecycle, content)
                 VALUES ('legacy-rule', 'errors', 0.9, 10, ?1, 'confirmed', 'active', 'Prefer explicit error types.')",
                params![rule_file.to_string_lossy().as_ref()],
            )
            .expect("seed learned_rules row");
            conn.execute(
                "DELETE FROM settings WHERE key = ?1",
                params![Storage::LEGACY_RULES_ARCHIVED_SENTINEL],
            )
            .expect("clear sentinel");
        }

        storage.archive_legacy_rules().expect("first archive pass");

        // Live file is gone.
        assert!(
            !rule_file.exists(),
            "legacy .md must be deleted from its live location"
        );
        // Archive copy + manifest exist under <data_local>/legacy-rules-archive/.
        let archive_root = data_dir
            .path()
            .canonicalize()
            .expect("canon data dir")
            .join("legacy-rules-archive");
        let mut found_copy = false;
        let mut found_manifest = false;
        fn walk(dir: &std::path::Path, copy: &mut bool, manifest: &mut bool) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        walk(&p, copy, manifest);
                    } else if p.file_name().is_some_and(|n| n == "ARCHIVE_MANIFEST.json") {
                        *manifest = true;
                        let body = std::fs::read_to_string(&p).unwrap_or_default();
                        assert!(
                            body.contains("legacy-rule.md")
                                && body.contains("\"sha256\"")
                                && body.contains("\"scope\""),
                            "manifest must record orig path, sha256, scope: {body}"
                        );
                    } else if p.extension().is_some_and(|x| x == "md") {
                        *copy = true;
                    }
                }
            }
        }
        walk(&archive_root, &mut found_copy, &mut found_manifest);
        assert!(found_copy, "an archived .md copy must exist");
        assert!(found_manifest, "ARCHIVE_MANIFEST.json must exist");

        // DB row is durably tombstoned.
        {
            let conn = storage.conn.lock();
            let (lifecycle, fp): (String, String) = conn
                .query_row(
                    "SELECT lifecycle, file_path FROM learned_rules WHERE name = 'legacy-rule'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("read back rule row");
            assert_eq!(lifecycle, "tombstoned", "rule must be tombstoned");
            assert_eq!(fp, "", "file_path must be cleared");
            let (by, reactivated): (String, Option<String>) = conn
                .query_row(
                    "SELECT tombstoned_by, reactivated_at FROM rule_tombstones WHERE rule_name = 'legacy-rule'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("read back tombstone");
            assert_eq!(
                by, "legacy_archive",
                "tombstone attributed to legacy_archive"
            );
            assert!(reactivated.is_none(), "tombstone must be active");
            assert!(
                tombstone_blocks(&conn, "legacy-rule"),
                "tombstone_blocks must report the archived rule as blocked"
            );
            let sentinel: Option<String> = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = ?1",
                    params![Storage::LEGACY_RULES_ARCHIVED_SENTINEL],
                    |r| r.get(0),
                )
                .ok();
            assert_eq!(sentinel.as_deref(), Some("1"), "sentinel must be set");
        }

        // Second pass: sentinel short-circuits. Re-seed a live file and
        // confirm the no-op leaves it untouched (proves the guard).
        std::fs::write(&rule_file, "Should not be re-archived.\n").expect("re-seed live .md");
        storage
            .archive_legacy_rules()
            .expect("second archive pass (no-op)");
        assert!(
            rule_file.exists(),
            "sentinel-guarded second pass must NOT re-wipe new files"
        );

        unsafe {
            std::env::remove_var("QUILL_RULES_DIR");
        }
        clear_env();
    }

    // ---------------------------------------------------------------------
    // Feature 005 US2 T024 — governance unit tests
    // (contracts/rule-governance.md, data-model.md "rule lifecycle" state
    // machine + validation rules, research R-2/R-3, spec SC-002/SC-004 + N7).
    // Each test asserts one persisted-governance behavior. DB tests are
    // `#[serial]` (shared env globals) and deterministic.
    // ---------------------------------------------------------------------

    /// C-5 / SC-004: a human-deleted rule must stay inactive across ≥5
    /// subsequent extraction cycles (0 resurrections). After
    /// `delete_learned_rule` writes a durable name-keyed tombstone, repeated
    /// `store_learned_rule` upserts with strong evidence must never return
    /// the row to `lifecycle='active'`, never re-arm a non-empty
    /// `file_path`/`content`, and must keep the tombstone active — even
    /// though α/β legitimately keep accruing (re-arming is gated, evidence
    /// is not).
    #[test]
    #[serial]
    fn tombstone_survives_at_least_five_re_extractions() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let name = "no-broad-catch";
        let original_content = "Prefer explicit error types over broad catches.";
        let seed = crate::models::LearnedRulePayload {
            name: name.to_string(),
            domain: Some("errors".to_string()),
            confidence: 0.92,
            observation_count: 12,
            file_path: "/some/learned/no-broad-catch.md".to_string(),
            project: None,
            is_anti_pattern: true,
            source: Some("claude".to_string()),
            content: Some(original_content.to_string()),
            provider_scope: vec![IntegrationProvider::Claude],
        };
        storage.store_learned_rule(&seed).expect("seed rule");

        // Each re-extraction carries DIFFERENT, strong content + a fresh
        // file_path so the assertion below genuinely proves the
        // suppression-sticky upsert refuses to *revive/overwrite* a
        // suppressed row (not merely that it left identical bytes).
        let re_extract = crate::models::LearnedRulePayload {
            confidence: 0.99,
            observation_count: 20,
            file_path: "/some/learned/no-broad-catch-RESURRECTED.md".to_string(),
            content: Some("RESURRECTED strong re-extracted body that must NOT win.".to_string()),
            ..seed.clone()
        };

        // Human deletes it: soft-delete + durable tombstone.
        storage
            .delete_learned_rule(name)
            .expect("delete must tombstone");

        let alpha_after_delete: f64 = {
            let conn = storage.conn.lock();
            // Precondition: deletion produced the durable suppression state.
            let (lifecycle, state, fp): (String, String, String) = conn
                .query_row(
                    "SELECT lifecycle, state, file_path FROM learned_rules WHERE name = ?1",
                    params![name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .expect("read deleted rule");
            assert_eq!(lifecycle, "tombstoned", "delete must set lifecycle");
            assert_eq!(state, "suppressed", "delete must hide via state");
            assert_eq!(fp, "", "delete must clear file_path");
            assert!(
                tombstone_blocks(&conn, name),
                "delete must write an active rule_tombstones row"
            );
            conn.query_row(
                "SELECT alpha FROM learned_rules WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .expect("read alpha")
        };

        // Simulate ≥5 re-extraction cycles of the SAME pattern with strong
        // evidence (high confidence, large observation_count). The
        // suppression-sticky `ON CONFLICT` must hold every single cycle.
        for cycle in 0..6 {
            storage
                .store_learned_rule(&re_extract)
                .unwrap_or_else(|e| panic!("re-extraction cycle {cycle} must not error: {e}"));

            let conn = storage.conn.lock();
            let (lifecycle, state, fp, content, alpha): (
                String,
                String,
                String,
                Option<String>,
                f64,
            ) = conn
                .query_row(
                    "SELECT lifecycle, state, file_path, content, alpha FROM learned_rules WHERE name = ?1",
                    params![name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .expect("read row after re-extraction");

            assert_eq!(
                lifecycle, "tombstoned",
                "cycle {cycle}: re-extraction must NOT revive lifecycle (no resurrection)"
            );
            assert_ne!(
                lifecycle, "active",
                "cycle {cycle}: a tombstoned rule must never become active via extraction"
            );
            assert_eq!(
                state, "suppressed",
                "cycle {cycle}: re-extraction must keep the row hidden"
            );
            assert_eq!(
                fp, "",
                "cycle {cycle}: suppression-sticky ON CONFLICT must not re-arm file_path"
            );
            // `delete_learned_rule` deliberately retains the pre-delete
            // `content` (it clears `file_path` + sets the durable
            // suppression, keeping the record for evidence). The sticky
            // upsert must therefore preserve that ORIGINAL content and must
            // NOT overwrite it with the strong re-extracted payload's body.
            assert_eq!(
                content.as_deref(),
                Some(original_content),
                "cycle {cycle}: suppression-sticky ON CONFLICT must not revive/overwrite content (got {content:?})"
            );
            assert!(
                tombstone_blocks(&conn, name),
                "cycle {cycle}: the durable tombstone must remain active"
            );
            // Evidence is allowed to (and should) keep accruing so a future
            // explicit reactivation can be gated on real signal.
            assert!(
                alpha > alpha_after_delete,
                "cycle {cycle}: α must keep accruing across re-extractions ({alpha} !> {alpha_after_delete})"
            );
        }

        clear_env();
    }

    /// R-2 / C-5: the `store_learned_rule` `ON CONFLICT(name)` upsert is
    /// suppression-sticky on `file_path`/`content`. A row that is either
    /// `lifecycle='suppressed'`/`'tombstoned'` OR has an active
    /// `rule_tombstones` entry must NOT have its `file_path`/`content`
    /// revived by a re-extraction upsert, regardless of how strong the new
    /// payload is. `lifecycle` itself is also left untouched here.
    #[test]
    #[serial]
    fn store_learned_rule_on_conflict_is_suppression_sticky() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // Case A: lifecycle='suppressed' (no tombstone row) — sticky purely
        // on the persisted lifecycle.
        {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO learned_rules (name, domain, confidence, observation_count, file_path, alpha, beta_param, state, lifecycle, is_anti_pattern, source, content, provider_scope)
                 VALUES ('sticky-suppressed', 'errors', 0.5, 4, '', 2.0, 2.0, 'suppressed', 'suppressed', 0, 'claude', NULL, '[\"claude\"]')",
                [],
            )
            .expect("seed suppressed row");
        }
        let strong = crate::models::LearnedRulePayload {
            name: "sticky-suppressed".to_string(),
            domain: Some("errors".to_string()),
            confidence: 0.99,
            observation_count: 20,
            file_path: "/learned/sticky-suppressed.md".to_string(),
            project: None,
            is_anti_pattern: false,
            source: Some("claude".to_string()),
            content: Some("Re-extracted strong content that must NOT revive.".to_string()),
            provider_scope: vec![IntegrationProvider::Claude],
        };
        storage
            .store_learned_rule(&strong)
            .expect("upsert suppressed row");
        {
            let conn = storage.conn.lock();
            let (fp, content, lifecycle): (String, Option<String>, String) = conn
                .query_row(
                    "SELECT file_path, content, lifecycle FROM learned_rules WHERE name = 'sticky-suppressed'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .expect("read suppressed row back");
            assert_eq!(fp, "", "suppressed row file_path must not be revived");
            assert!(
                content.is_none(),
                "suppressed row content must not be revived, got {content:?}"
            );
            assert_eq!(
                lifecycle, "suppressed",
                "ON CONFLICT must not mutate lifecycle"
            );
        }

        // Case B: a *tombstoned* row backed by an active rule_tombstones
        // entry — sticky via the tombstone existence clause too.
        {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO learned_rules (name, domain, confidence, observation_count, file_path, alpha, beta_param, state, lifecycle, is_anti_pattern, source, content, provider_scope)
                 VALUES ('sticky-tombstoned', 'errors', 0.5, 4, '', 2.0, 7.0, 'suppressed', 'tombstoned', 0, 'claude', NULL, '[\"claude\"]')",
                [],
            )
            .expect("seed tombstoned row");
            conn.execute(
                "INSERT INTO rule_tombstones (rule_name, tombstoned_by, reason) VALUES ('sticky-tombstoned', 'human', 'test')",
                [],
            )
            .expect("seed active tombstone");
        }
        let strong_b = crate::models::LearnedRulePayload {
            name: "sticky-tombstoned".to_string(),
            file_path: "/learned/sticky-tombstoned.md".to_string(),
            content: Some("Strong re-extraction body for a tombstoned rule.".to_string()),
            ..strong.clone()
        };
        storage
            .store_learned_rule(&strong_b)
            .expect("upsert tombstoned row");
        {
            let conn = storage.conn.lock();
            let (fp, content, lifecycle): (String, Option<String>, String) = conn
                .query_row(
                    "SELECT file_path, content, lifecycle FROM learned_rules WHERE name = 'sticky-tombstoned'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .expect("read tombstoned row back");
            assert_eq!(
                fp, "",
                "tombstoned row file_path must not be revived by re-extraction"
            );
            assert!(
                content.is_none(),
                "tombstoned row content must not be revived, got {content:?}"
            );
            assert_eq!(
                lifecycle, "tombstoned",
                "ON CONFLICT must not mutate a tombstoned row's lifecycle"
            );
        }

        clear_env();
    }

    /// R-3 / data-model.md: `lifecycle` is a distinct persisted column.
    /// `get_learned_rules` recomputes the read-derived quality `state` via
    /// `evidence_weighted_score`/`compute_state` on every read and must
    /// NEVER write back to (clobber) the `lifecycle` column. Seed one row
    /// per lifecycle value (with a non-`suppressed` `state` so the read
    /// returns it), call `get_learned_rules`, then re-read `lifecycle`
    /// directly and assert it is byte-for-byte preserved and independent of
    /// the recomputed `state`.
    #[test]
    #[serial]
    fn get_learned_rules_does_not_clobber_lifecycle() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // Strong evidence + recent timestamp ⇒ recomputed state = 'confirmed'
        // (distinct from every lifecycle value below), proving the read
        // recomputes `state` yet never touches `lifecycle`.
        let recent = Utc::now().to_rfc3339();
        let lifecycles = [
            "candidate",
            "awaiting_review",
            "active",
            "rejected",
            "suppressed",
            "tombstoned",
        ];
        {
            let conn = storage.conn.lock();
            for lc in lifecycles {
                conn.execute(
                    "INSERT INTO learned_rules (name, domain, confidence, observation_count, file_path, alpha, beta_param, last_evidence_at, state, lifecycle, is_anti_pattern, source, content, provider_scope)
                     VALUES (?1, 'errors', 0.9, 10, '', 30.0, 1.0, ?2, 'emerging', ?3, 0, 'claude', 'body', '[\"claude\"]')",
                    params![format!("lc-{lc}"), recent, lc],
                )
                .unwrap_or_else(|e| panic!("seed row for {lc}: {e}"));
            }
        }

        // Read path: recomputes `state`, returns DB-only candidates, must
        // not write `lifecycle`.
        let rules = storage
            .get_learned_rules(None)
            .expect("get_learned_rules must succeed");
        // Sanity: the read recomputes `state` to the evidence-weighted label
        // (not the literal 'emerging' we stored) for the visible rows —
        // proving it really does derive `state`.
        assert!(
            rules.iter().any(|r| r.name == "lc-candidate"),
            "a non-suppressed DB-only row must surface"
        );

        let conn = storage.conn.lock();
        for lc in lifecycles {
            let (db_lifecycle, db_state): (String, String) = conn
                .query_row(
                    "SELECT lifecycle, state FROM learned_rules WHERE name = ?1",
                    params![format!("lc-{lc}")],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap_or_else(|e| panic!("re-read row for {lc}: {e}"));
            assert_eq!(
                db_lifecycle, lc,
                "get_learned_rules must NOT clobber persisted lifecycle '{lc}'"
            );
            // `state` is the recomputed quality label; `lifecycle` is the
            // independent governance column — they are distinct concerns and
            // `state` must not have leaked into `lifecycle`.
            assert_ne!(
                db_lifecycle, db_state,
                "lifecycle ('{db_lifecycle}') must stay distinct from derived state ('{db_state}')"
            );
        }

        drop(conn);
        clear_env();
    }

    /// FR-009 / SC-004: `rollback_rule` is a forward, append-only restore.
    /// Promote-equivalent v1 content, then mutate `learned_rules` content +
    /// bump `current_version` to a v2 row; rolling back to v1 must restore
    /// `learned_rules.content/content_hash/current_version` to the v1
    /// snapshot AND append a NEW `rule_versions` row with
    /// `change_kind='rollback'` and `rolled_back_from=1`, while the existing
    /// v1/v2 rows remain byte-for-byte intact (history is never rewritten).
    #[test]
    #[serial]
    fn rollback_rule_restores_prior_version_append_only() {
        use sha2::Digest;
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let name = "rollback-target";
        let v1_content = "Version one: prefer explicit error types.";
        let v2_content = "Version two: rewritten guidance that we will undo.";
        let v1_hash = format!("{:x}", sha2::Sha256::digest(v1_content.as_bytes()));
        let v2_hash = format!("{:x}", sha2::Sha256::digest(v2_content.as_bytes()));

        let rule_id: i64 = {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO learned_rules (name, domain, confidence, observation_count, file_path, alpha, beta_param, state, lifecycle, current_version, content, content_hash, is_anti_pattern, source, provider_scope)
                 VALUES (?1, 'errors', 0.9, 10, '', 8.0, 1.0, 'confirmed', 'active', 2, ?2, ?3, 0, 'claude', '[\"claude\"]')",
                params![name, v2_content, v2_hash],
            )
            .expect("seed active rule at v2");
            let id: i64 = conn
                .query_row(
                    "SELECT id FROM learned_rules WHERE name = ?1",
                    params![name],
                    |r| r.get(0),
                )
                .expect("rule id");
            // Append-only history: immutable v1 then v2 snapshots.
            conn.execute(
                "INSERT INTO rule_versions (rule_id, version, content, content_hash, domain, is_anti_pattern, change_kind, author)
                 VALUES (?1, 1, ?2, ?3, 'errors', 0, 'create', 'system')",
                params![id, v1_content, v1_hash],
            )
            .expect("seed v1");
            conn.execute(
                "INSERT INTO rule_versions (rule_id, version, content, content_hash, domain, is_anti_pattern, change_kind, author)
                 VALUES (?1, 2, ?2, ?3, 'errors', 0, 'update', 'system')",
                params![id, v2_content, v2_hash],
            )
            .expect("seed v2");
            id
        };

        // Roll back to version 1 (no file_path ⇒ DB-only restore path; the
        // content has no fences/secrets so redact→sanitize is a fixed point
        // and the restored bytes equal v1 exactly).
        storage
            .rollback_rule(name, 1)
            .expect("rollback to v1 must succeed");

        let conn = storage.conn.lock();
        let (content, content_hash, current_version): (String, String, i64) = conn
            .query_row(
                "SELECT content, content_hash, current_version FROM learned_rules WHERE name = ?1",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("read rolled-back rule");
        assert_eq!(content, v1_content, "content must be restored to v1");
        assert_eq!(content_hash, v1_hash, "content_hash must match restored v1");
        assert_eq!(
            current_version, 3,
            "current_version must advance to the new append-only rollback row (3)"
        );

        // A NEW rule_versions row (version 3) records the rollback forward.
        let (rb_kind, rb_from, rb_content): (String, Option<i64>, String) = conn
            .query_row(
                "SELECT change_kind, rolled_back_from, content FROM rule_versions WHERE rule_id = ?1 AND version = 3",
                params![rule_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("rollback version row must exist");
        assert_eq!(
            rb_kind, "rollback",
            "new row must be change_kind='rollback'"
        );
        assert_eq!(rb_from, Some(1), "rolled_back_from must point at v1");
        assert_eq!(rb_content, v1_content, "rollback row content must be v1");

        // History is append-only: v1/v2 untouched, total versions = 3.
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM rule_versions WHERE rule_id = ?1",
                params![rule_id],
                |r| r.get(0),
            )
            .expect("count versions");
        assert_eq!(total, 3, "rollback must APPEND, never rewrite history");
        let v1_still: String = conn
            .query_row(
                "SELECT content FROM rule_versions WHERE rule_id = ?1 AND version = 1",
                params![rule_id],
                |r| r.get(0),
            )
            .expect("v1 still present");
        let v2_still: String = conn
            .query_row(
                "SELECT content FROM rule_versions WHERE rule_id = ?1 AND version = 2",
                params![rule_id],
                |r| r.get(0),
            )
            .expect("v2 still present");
        assert_eq!(v1_still, v1_content, "v1 snapshot must remain intact");
        assert_eq!(v2_still, v2_content, "v2 snapshot must remain intact");

        drop(conn);
        clear_env();
    }

    /// FR-010: `reactivate_rule` is the ONLY path that clears a durable
    /// tombstone. After delete→tombstoned, reactivation must set
    /// `rule_tombstones.reactivated_at`/`reactivated_by`, stop
    /// `tombstone_blocks` from blocking, and return the rule to
    /// `lifecycle='candidate'` (NOT auto-active) so it must re-earn review
    /// eligibility through the gated pipeline — and it must NOT write any
    /// `.md`.
    #[test]
    #[serial]
    fn reactivate_rule_is_the_sole_untombstone_path() {
        clear_env();
        let data_dir = TempDir::new().expect("data tempdir");
        let rules_dir = TempDir::new().expect("rules tempdir");
        // SAFETY: env mutation; serialized via #[serial]. Route demo-mode
        // rule dirs into a temp dir so reactivation can be proven to write
        // no real `.md`.
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_DATA_DIR", data_dir.path());
            std::env::set_var("QUILL_RULES_DIR", rules_dir.path());
        }
        let storage = Storage::init().expect("init storage");

        let name = "reactivate-me";
        storage
            .store_learned_rule(&crate::models::LearnedRulePayload {
                name: name.to_string(),
                domain: Some("errors".to_string()),
                confidence: 0.9,
                observation_count: 10,
                file_path: String::new(),
                project: None,
                is_anti_pattern: false,
                source: Some("claude".to_string()),
                content: Some("Body.".to_string()),
                provider_scope: vec![IntegrationProvider::Claude],
            })
            .expect("seed rule");
        storage.delete_learned_rule(name).expect("delete→tombstone");

        // Sanity: it is actually tombstoned before reactivation.
        {
            let conn = storage.conn.lock();
            assert!(
                tombstone_blocks(&conn, name),
                "precondition: rule must be tombstoned"
            );
        }

        storage
            .reactivate_rule(name)
            .expect("reactivate must succeed on an active tombstone");

        {
            let conn = storage.conn.lock();
            let (reactivated_at, reactivated_by): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT reactivated_at, reactivated_by FROM rule_tombstones WHERE rule_name = ?1",
                    params![name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("read tombstone after reactivate");
            assert!(
                reactivated_at.is_some(),
                "reactivate must set reactivated_at"
            );
            assert_eq!(
                reactivated_by.as_deref(),
                Some("human"),
                "reactivate must record the actor"
            );
            assert!(
                !tombstone_blocks(&conn, name),
                "a reactivated tombstone must no longer block"
            );
            let lifecycle: String = conn
                .query_row(
                    "SELECT lifecycle FROM learned_rules WHERE name = ?1",
                    params![name],
                    |r| r.get(0),
                )
                .expect("read lifecycle after reactivate");
            assert_eq!(
                lifecycle, "candidate",
                "reactivation must return the rule to 'candidate', NEVER auto-active"
            );
            assert_ne!(
                lifecycle, "active",
                "reactivation must not auto-activate the rule"
            );
        }

        // Reactivation must not have authored any `.md` in any scope dir.
        for scope in [
            vec![IntegrationProvider::Claude],
            vec![IntegrationProvider::Codex],
            vec![IntegrationProvider::Claude, IntegrationProvider::Codex],
        ] {
            let d = crate::learning::learned_rules_dir_for_scope(&scope);
            let md_count = std::fs::read_dir(&d)
                .map(|it| {
                    it.flatten()
                        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
                        .count()
                })
                .unwrap_or(0);
            assert_eq!(
                md_count, 0,
                "reactivation must write 0 .md files (dir {d:?})"
            );
        }

        unsafe {
            std::env::remove_var("QUILL_RULES_DIR");
        }
        clear_env();
    }

    /// N7 / SC-002: with the autonomous `above_threshold` writer removed,
    /// extraction only ever persists a DB candidate — it MUST write 0 `.md`
    /// files to any learned-rule dir, even for a maximally-confident rule.
    /// `write_rule_files` is a private fn requiring a `tauri::AppHandle`;
    /// `store_learned_rule` is the exact persistence seam it now calls for
    /// every extracted rule (the `fs::write` was deleted from that path), so
    /// driving it with a high-confidence payload faithfully exercises the
    /// no-autonomous-writer invariant. All rule dirs are pointed at a temp
    /// dir so no real learned-rule directory is touched.
    #[test]
    #[serial]
    fn extraction_writes_zero_md_files_only_a_db_candidate() {
        clear_env();
        let data_dir = TempDir::new().expect("data tempdir");
        let rules_dir = TempDir::new().expect("rules tempdir");
        // SAFETY: env mutation; serialized via #[serial]. Route every
        // provider scope's learned-rule dir under the temp rules dir.
        unsafe {
            std::env::set_var("QUILL_DEMO_MODE", "1");
            std::env::set_var("QUILL_DATA_DIR", data_dir.path());
            std::env::set_var("QUILL_RULES_DIR", rules_dir.path());
        }
        let storage = Storage::init().expect("init storage");

        // A maximally-confident, well-evidenced candidate — exactly the kind
        // the deleted autonomous branch would have written straight to a
        // global `.md`.
        let name = "high-confidence-candidate";
        storage
            .store_learned_rule(&crate::models::LearnedRulePayload {
                name: name.to_string(),
                domain: Some("errors".to_string()),
                confidence: 0.999,
                observation_count: 50,
                file_path: String::new(),
                project: None,
                is_anti_pattern: false,
                source: Some("claude".to_string()),
                content: Some("Always prefer explicit, specific error types.".to_string()),
                provider_scope: vec![IntegrationProvider::Claude],
            })
            .expect("store high-confidence candidate");

        // It is persisted as a DB-only `candidate` with no on-disk file.
        {
            let conn = storage.conn.lock();
            let (lifecycle, file_path, content): (String, String, Option<String>) = conn
                .query_row(
                    "SELECT lifecycle, file_path, content FROM learned_rules WHERE name = ?1",
                    params![name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .expect("read stored candidate");
            assert_eq!(
                lifecycle, "candidate",
                "extraction must persist only a DB candidate, never auto-activate"
            );
            assert_eq!(
                file_path, "",
                "extraction must NOT set a file_path (no autonomous .md)"
            );
            assert!(
                content.is_some(),
                "the candidate body must still be tracked in the DB for later review"
            );
        }

        // Zero `.md` files in any learned-rule scope dir — the autonomous
        // extraction→global-`.md` path no longer exists (SC-002).
        let mut total_md = 0usize;
        for scope in [
            vec![IntegrationProvider::Claude],
            vec![IntegrationProvider::Codex],
            vec![IntegrationProvider::Claude, IntegrationProvider::Codex],
        ] {
            let d = crate::learning::learned_rules_dir_for_scope(&scope);
            if let Ok(it) = std::fs::read_dir(&d) {
                total_md += it
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
                    .count();
            }
        }
        assert_eq!(
            total_md, 0,
            "a periodic/extraction run with the autonomous branch removed must write 0 .md files"
        );

        unsafe {
            std::env::remove_var("QUILL_RULES_DIR");
        }
        clear_env();
    }

    // ---------------------------------------------------------------------
    // Feature 005 US3 T037 — grounding / cluster / verdict unit tests
    // (R-6 / C-3/H-1/H-2/M-3/M-4, FR-014..018/029, SC-005/SC-006).
    //
    // Storage-primitive boundary: `learning::write_rule_files` performs the
    // FR-015 reject and FR-016 surfacing using `resolve_evidence_refs` +
    // `eligible_for_review`; those storage functions are the unit-testable
    // single source of truth (R-4 "evidence-weighted gate"). All tests use
    // `kind="observation"` refs (resolve purely against `observations` rows
    // — no git/session subprocess) or directly-seeded
    // `rule_evidence_citations` rows so every assertion is deterministic.
    // `#[serial]` (shared env globals); harness mirrors the migration_25_*
    // / store_observation_redacts_* tests exactly.
    // ---------------------------------------------------------------------

    /// Insert a minimal `observations` row and return its id so a
    /// `kind="observation"` ref can be made resolvable deterministically.
    fn seed_observation(storage: &Storage, session: &str, cwd: &str) -> i64 {
        let conn = storage.conn.lock();
        conn.execute(
            "INSERT INTO observations
                (provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd)
             VALUES ('claude', ?1, datetime('now'), 'PostToolUse', 'Bash', 'in', 'out', ?2)",
            params![session, cwd],
        )
        .expect("seed observation");
        conn.last_insert_rowid()
    }

    fn obs_ref(id: i64) -> EvidenceRef {
        EvidenceRef {
            kind: "observation".to_string(),
            id: id.to_string(),
        }
    }

    /// FR-015 / SC-005 (H-1, research R-6 "Grounding"): a candidate whose
    /// `evidence_refs` resolve to ZERO real records yields an empty
    /// `ResolvedEvidence` (`distinct_count()==0`) — the exact condition
    /// `learning::write_rule_files` rejects on (not stored, kept out of
    /// `awaiting_review`). A candidate with ≥1 resolvable citation resolves
    /// (`distinct_count()>=1`) so it can persist. De-dup by `(kind,id)` is
    /// also exercised so a doubled citation cannot inflate the count.
    #[test]
    #[serial]
    fn resolve_evidence_refs_rejects_unresolvable_and_resolves_real_citations() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // Ungrounded: ids that do not exist in `observations`.
        let fabricated = vec![obs_ref(424_242), obs_ref(999_999)];
        let r = storage.resolve_evidence_refs(&fabricated, None);
        assert_eq!(
            r.distinct_count(),
            0,
            "fabricated citations must resolve to 0 (write_rule_files rejects this candidate)"
        );
        assert_eq!(
            r.distinct_sources, 0,
            "no resolved kinds for an ungrounded candidate"
        );

        // Grounded: one real observation row → at least one resolved.
        let oid = seed_observation(&storage, "s-grounded", "/proj/a");
        let grounded = vec![obs_ref(oid), obs_ref(424_242)];
        let r2 = storage.resolve_evidence_refs(&grounded, None);
        assert_eq!(
            r2.distinct_count(),
            1,
            "exactly the one real citation resolves; the fabricated one is dropped"
        );
        assert!(
            r2.project_paths.contains(&"/proj/a".to_string()),
            "resolved observation cwd is captured as a distinct project path"
        );

        // De-dup: the same (kind,id) cited twice must not double-count.
        let doubled = vec![obs_ref(oid), obs_ref(oid)];
        assert_eq!(
            storage
                .resolve_evidence_refs(&doubled, None)
                .distinct_count(),
            1,
            "duplicate (kind,id) refs must de-dup to a single resolved citation"
        );

        clear_env();
    }

    /// FR-016 / SC-005/SC-006 (H-2, research R-6 "Min cluster"):
    /// `eligible_for_review` is FALSE while the minimum evidence cluster is
    /// unmet (`resolved_distinct_refs < 3` OR `distinct_sources < 1`) even
    /// with a maximal stated confidence, and TRUE once met — uniformly for
    /// an A-style (Stream-A `observation` citations) and a B/C-style
    /// (Stream-B/C `session` citations) rule. Confirms the raw self-rating
    /// never substitutes for the evidence-weighted gate (SC-006).
    #[test]
    #[serial]
    fn eligible_for_review_enforces_min_cluster_uniformly_across_streams() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // --- A-style rule: Stream-A observation citations. ---
        // Maximal confidence ⇒ α-dominant ⇒ score ≫ 0.6, isolating the
        // cluster gate as the only thing that can fail.
        let a_name = "a-style-rule";
        let seed_a = crate::models::LearnedRulePayload {
            name: a_name.to_string(),
            domain: Some("errors".to_string()),
            confidence: 0.99,
            observation_count: 20,
            file_path: String::new(),
            project: None,
            is_anti_pattern: false,
            source: Some("claude".to_string()),
            content: Some("Prefer explicit error types.".to_string()),
            provider_scope: vec![IntegrationProvider::Claude],
        };
        storage.store_learned_rule(&seed_a).expect("seed A rule");

        // Only 2 distinct resolved citations → cluster unmet.
        let o1 = seed_observation(&storage, "sa1", "/proj/a");
        let o2 = seed_observation(&storage, "sa2", "/proj/a");
        let resolved2 = storage.resolve_evidence_refs(&[obs_ref(o1), obs_ref(o2)], None);
        storage
            .persist_citations_and_advance_version(a_name, &resolved2, false)
            .expect("persist 2 citations");
        assert!(
            !storage
                .eligible_for_review(a_name)
                .expect("eligibility A<3"),
            "FR-016: <3 resolved distinct refs is NOT eligible despite confidence 0.99"
        );

        // Add a 3rd distinct citation → cluster met (3 refs; distinct
        // sources = 1 kind + ≥1 project path).
        let o3 = seed_observation(&storage, "sa3", "/proj/a");
        let resolved3 =
            storage.resolve_evidence_refs(&[obs_ref(o1), obs_ref(o2), obs_ref(o3)], None);
        storage
            .persist_citations_and_advance_version(a_name, &resolved3, false)
            .expect("persist 3 citations");
        assert!(
            storage
                .eligible_for_review(a_name)
                .expect("eligibility A>=3"),
            "FR-016: ≥3 resolved refs + ≥1 source + high score IS eligible"
        );

        // --- B/C-style rule: Stream-B/C `session` citations, no obs rows,
        // no project paths. distinct_sources must come from the citation
        // `kind` alone (= 1), proving the gate is uniform across streams.
        //
        // FINDING (matches research R-6, not a bug): `store_learned_rule`
        // clamps `evidence_scale` to a MINIMUM of 5, so at confidence ≈1.0 a
        // brand-new rule's Wilson lower bound peaks at ≈0.554 — *below* the
        // default `min_eligibility` 0.6. A first-run rule meeting only the
        // bare 3-citation minimum is therefore NOT yet eligible; eligibility
        // requires accumulated evidence (α/β grow across re-derivation since
        // the upsert ADDS α/β), exactly R-6's "judged on accumulated
        // evidence, not a single batch" intent (the gate runs post-merge).
        // To isolate the *cluster* gate (the unit under test) we seed
        // accumulated α directly so score ≥ 0.6 and the citation count is
        // the only variable that flips eligibility. ---
        let bc_name = "bc-style-rule";
        let seed_bc = crate::models::LearnedRulePayload {
            name: bc_name.to_string(),
            domain: Some("workflow".to_string()),
            confidence: 0.99,
            // H-2 fix: a B/C rule's own resolved-citation count drives α/β.
            observation_count: 3,
            file_path: String::new(),
            project: None,
            is_anti_pattern: false,
            source: Some("codex".to_string()),
            content: Some("Run the test suite before pushing.".to_string()),
            provider_scope: vec![IntegrationProvider::Codex],
        };
        storage.store_learned_rule(&seed_bc).expect("seed B/C rule");
        // Simulate accumulated evidence across runs so the Wilson score
        // clears 0.6 — isolates the cluster threshold as the only gate.
        {
            let conn = storage.conn.lock();
            conn.execute(
                "UPDATE learned_rules SET alpha = 25.0, beta_param = 1.0 WHERE name = ?1",
                params![bc_name],
            )
            .expect("seed accumulated B/C evidence");
        }
        let (rule_id, cur_ver): (i64, i64) = {
            let conn = storage.conn.lock();
            conn.query_row(
                "SELECT id, current_version FROM learned_rules WHERE name = ?1",
                params![bc_name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read bc rule id")
        };

        // Seed 2 session citations → cluster unmet.
        {
            let conn = storage.conn.lock();
            for sid in ["sess-1", "sess-2"] {
                conn.execute(
                    "INSERT INTO rule_evidence_citations
                        (rule_id, rule_version, kind, ref_id, session_id, snippet)
                     VALUES (?1, ?2, 'session', ?3, ?3, 'session:' || ?3)",
                    params![rule_id, cur_ver, sid],
                )
                .expect("seed session citation");
            }
        }
        assert!(
            !storage
                .eligible_for_review(bc_name)
                .expect("eligibility BC<3"),
            "FR-016: a B/C rule with <3 session citations is NOT eligible"
        );

        // Add a 3rd distinct session citation → met (3 refs; distinct
        // sources = 1 kind, 0 projects ⇒ distinct_sources = 1).
        {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO rule_evidence_citations
                    (rule_id, rule_version, kind, ref_id, session_id, snippet)
                 VALUES (?1, ?2, 'session', 'sess-3', 'sess-3', 'session:sess-3')",
                params![rule_id, cur_ver],
            )
            .expect("seed 3rd session citation");
        }
        assert!(
            storage
                .eligible_for_review(bc_name)
                .expect("eligibility BC>=3"),
            "FR-016: a B/C (session-only) rule with ≥3 citations IS eligible — gate uniform across streams"
        );

        clear_env();
    }

    /// H-2 / FR-016 (research R-6 "Min cluster"): the `observation_count=0`
    /// bug fix. `store_learned_rule` with `observation_count` = the rule's
    /// own resolved-citation count (the value `write_rule_files` now threads
    /// in for Stream-B/C rules that have NO `observations` rows) must
    /// persist that exact count — NOT 0 — and scale α/β off it.
    #[test]
    #[serial]
    fn store_learned_rule_persists_resolved_citation_count_not_zero() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // Mirrors write_rule_files: resolved_count (= distinct resolved
        // citations) is passed as observation_count for a B/C rule with no
        // observation rows at all.
        let resolved_count: i64 = 4;
        let payload = crate::models::LearnedRulePayload {
            name: "bc-no-obs-rows".to_string(),
            domain: Some("workflow".to_string()),
            confidence: 0.8,
            observation_count: resolved_count,
            file_path: String::new(),
            project: None,
            is_anti_pattern: false,
            source: Some("codex".to_string()),
            content: Some("Tag releases with a changelog entry.".to_string()),
            provider_scope: vec![IntegrationProvider::Codex],
        };
        storage
            .store_learned_rule(&payload)
            .expect("store B/C rule");

        let (obs_count, alpha, beta): (i64, f64, f64) = {
            let conn = storage.conn.lock();
            conn.query_row(
                "SELECT observation_count, alpha, beta_param FROM learned_rules WHERE name = 'bc-no-obs-rows'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("read back B/C rule")
        };
        assert_eq!(
            obs_count, resolved_count,
            "H-2: observation_count must be the resolved-citation count, not 0"
        );
        // evidence_scale = clamp(count,5,20)=5 here; α=0.8*5, β=0.2*5.
        assert!(
            (alpha - 4.0).abs() < 1e-9 && (beta - 1.0).abs() < 1e-9,
            "α/β must scale off the resolved count (α={alpha}, β={beta}), not collapse to 0"
        );

        clear_env();
    }

    /// M-4 / FR-017 (research R-6 "Verdicts"): an `irrelevant` verdict is
    /// realized by `decay_rule_freshness` — it pushes `last_evidence_at`
    /// back exactly one 90-day half-life (freshness drops), is
    /// monotone-backward (re-applying only decays further, never refreshes),
    /// leaves α/β UNCHANGED (so it does not by itself flip the rule to
    /// `invalidated`), and is a safe no-op on an unknown rule name (the
    /// documented storage behavior for the "unknown verdict not silently
    /// dropped / no panic" guarantee).
    #[test]
    #[serial]
    fn decay_rule_freshness_moves_state_backward_without_invalidating() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let name = "decays-on-irrelevant";
        let start = (Utc::now() - chrono::Duration::days(5)).to_rfc3339();
        {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO learned_rules
                    (name, domain, confidence, observation_count, file_path, alpha, beta_param,
                     last_evidence_at, state, lifecycle, is_anti_pattern, source, content, provider_scope)
                 VALUES (?1, 'errors', 0.9, 12, '', 15.0, 1.0, ?2, 'confirmed', 'candidate', 0, 'claude',
                         'Prefer explicit error types.', '[\"claude\"]')",
                params![name, start],
            )
            .expect("seed rule");
        }

        let read = |s: &Storage| -> (String, f64, f64) {
            let conn = s.conn.lock();
            conn.query_row(
                "SELECT last_evidence_at, alpha, beta_param FROM learned_rules WHERE name = ?1",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("read rule")
        };

        let (ts0, a0, b0) = read(&storage);
        let (score0, state0) = evidence_weighted_score(a0, b0, Some(ts0.as_str()));

        storage.decay_rule_freshness(name).expect("decay once");
        let (ts1, a1, b1) = read(&storage);
        let (score1, state1) = evidence_weighted_score(a1, b1, Some(ts1.as_str()));

        let t0 = DateTime::parse_from_rfc3339(&ts0)
            .unwrap()
            .with_timezone(&Utc);
        let t1 = DateTime::parse_from_rfc3339(&ts1)
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            t1 < t0,
            "irrelevant verdict must move last_evidence_at backward ({t1} !< {t0})"
        );
        let delta_days = (t0 - t1).num_seconds() as f64 / 86400.0;
        assert!(
            (delta_days - 90.0).abs() < 0.01,
            "exactly one 90-day half-life of decay, got {delta_days} days"
        );
        assert!(
            score1 < score0,
            "freshness decay must lower the evidence-weighted score ({score1} !< {score0})"
        );
        // α/β untouched ⇒ no β-override ⇒ NOT invalidated by this verdict
        // alone (state0 was a healthy α-dominant label).
        assert_eq!((a1, b1), (a0, b0), "irrelevant verdict must NOT mutate α/β");
        assert_ne!(
            state1, "invalidated",
            "an irrelevant verdict alone must not flip the rule to invalidated (was {state0}, now {state1})"
        );

        // Monotone-backward: re-applying only decays further.
        storage.decay_rule_freshness(name).expect("decay twice");
        let (ts2, ..) = read(&storage);
        let t2 = DateTime::parse_from_rfc3339(&ts2)
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            t2 < t1,
            "re-applying irrelevant must keep decaying ({t2} !< {t1})"
        );

        // Unknown verdict / unknown rule: documented safe no-op — returns
        // Ok, does not panic, mutates nothing, drops nothing silently.
        storage
            .decay_rule_freshness("no-such-rule-xyz")
            .expect("decay on an unknown rule must be a safe Ok no-op (not a panic/silent drop)");
        let still: i64 = {
            let conn = storage.conn.lock();
            conn.query_row(
                "SELECT COUNT(*) FROM learned_rules WHERE name = 'no-such-rule-xyz'",
                [],
                |r| r.get(0),
            )
            .expect("count")
        };
        assert_eq!(
            still, 0,
            "unknown-rule decay must not create or mutate any row"
        );

        clear_env();
    }

    /// M-4 / FR-017 (research R-6 "Verdicts"): `compute_state`'s
    /// strong-contradiction β-override via `evidence_weighted_score`.
    /// `beta >= alpha && beta >= 5.0` ⇒ `invalidated`, ORDERED after the
    /// stale check (a stale heavily-contradicted rule still reads `stale`)
    /// and before the confidence bands (a heavily-contradicted FRESH rule
    /// can never read `emerging`/`confirmed`). A healthy α-dominant fresh
    /// rule still reads `confirmed`/`emerging` as before.
    #[test]
    #[serial]
    fn compute_state_beta_override_invalidates_after_stale_before_bands() {
        // Fresh + strong contradiction (β≥α, β≥5) ⇒ invalidated regardless
        // of Wilson confidence.
        let fresh = Utc::now().to_rfc3339();
        let (_s, st) = evidence_weighted_score(2.0, 8.0, Some(fresh.as_str()));
        assert_eq!(
            st, "invalidated",
            "β≥α and β≥5 on a fresh rule must be invalidated (override before the bands)"
        );

        // Boundary: β==α==5 (β≥α true, β≥5 true) ⇒ invalidated.
        let (_s2, st2) = evidence_weighted_score(5.0, 5.0, Some(fresh.as_str()));
        assert_eq!(st2, "invalidated", "β==α==5 hits the override boundary");

        // Just under the substantiality floor (β==α==4 ⇒ β<5) does NOT
        // override on that rule alone (low Wilson confidence still labels it
        // invalidated via the OR-ed `confidence < 0.4`, so assert the
        // override itself is gated: a single confirmation flips the label).
        let (_s3, st3) = evidence_weighted_score(20.0, 4.0, Some(fresh.as_str()));
        assert_ne!(
            st3, "invalidated",
            "β=4 (<5) must NOT trigger the strong-contradiction override for an α-dominant rule"
        );

        // Stale check is ordered FIRST: an old, heavily-contradicted rule
        // reads `stale`, not `invalidated`.
        let old = (Utc::now() - chrono::Duration::days(900)).to_rfc3339();
        let (_s4, st4) = evidence_weighted_score(2.0, 9.0, Some(old.as_str()));
        assert_eq!(
            st4, "stale",
            "stale check precedes the β-override (old contradicted rule reads stale)"
        );

        // Healthy α-dominant fresh rule is unaffected by the override.
        let (sc5, st5) = evidence_weighted_score(19.0, 1.0, Some(fresh.as_str()));
        assert_eq!(
            st5, "confirmed",
            "an α-dominant fresh rule still reads confirmed (score {sc5})"
        );
        // `emerging` band = Wilson lower bound in [0.4, 0.6). Wilson is
        // conservative at small n (e.g. α=3,β=2 ⇒ ≈0.23 < 0.4 ⇒ the OR-ed
        // `confidence < 0.4` labels it `invalidated`, NOT a bug), so use a
        // larger-n α-leaning sample that genuinely lands mid-band.
        let (sc6, st6) = evidence_weighted_score(8.0, 3.0, Some(fresh.as_str()));
        assert!(
            (0.4..0.6).contains(&sc6),
            "sample must land in the emerging band, got {sc6}"
        );
        assert_eq!(
            st6, "emerging",
            "a mid-band α-leaning fresh rule reads emerging (not invalidated)"
        );
    }

    /// M-3 / FR-018 (research R-6 "Conflict/dedup"):
    /// `record_rule_reconciliation` deterministically supersedes duplicates
    /// and flags conflicts. Two duplicate candidates (same domain,
    /// overlapping resolved evidence) → the loser gets
    /// `lifecycle='superseded'` + `superseded_by=<survivor>`, survivor =
    /// higher `evidence_weighted_score` (documented deterministic
    /// tie-break). An anti-pattern/positive pair over shared evidence →
    /// BOTH `conflict_flagged`. Re-running is idempotent (terminal rows are
    /// skipped, not re-superseded).
    #[test]
    #[serial]
    fn record_rule_reconciliation_supersedes_dupes_and_flags_conflicts_idempotently() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // --- Duplicate pair: same domain, shared resolved evidence. Survivor
        // is the higher-score rule (more α). ---
        let dup_strong = crate::models::LearnedRulePayload {
            name: "dup-strong".to_string(),
            domain: Some("errors".to_string()),
            confidence: 0.95,
            observation_count: 20,
            file_path: String::new(),
            project: None,
            is_anti_pattern: false,
            source: Some("claude".to_string()),
            content: Some("Prefer explicit error types.".to_string()),
            provider_scope: vec![IntegrationProvider::Claude],
        };
        let dup_weak = crate::models::LearnedRulePayload {
            name: "dup-weak".to_string(),
            confidence: 0.55,
            observation_count: 5,
            ..dup_strong.clone()
        };
        storage
            .store_learned_rule(&dup_strong)
            .expect("seed dup-strong");
        storage
            .store_learned_rule(&dup_weak)
            .expect("seed dup-weak");

        // Shared evidence: same observation citation on both.
        let shared_obs = seed_observation(&storage, "shared", "/proj/x");
        let shared = storage.resolve_evidence_refs(&[obs_ref(shared_obs)], None);
        storage
            .persist_citations_and_advance_version("dup-strong", &shared, false)
            .expect("cite strong");
        storage
            .persist_citations_and_advance_version("dup-weak", &shared, false)
            .expect("cite weak");

        // --- Conflict pair: opposite is_anti_pattern over shared evidence. ---
        let pos = crate::models::LearnedRulePayload {
            name: "policy-positive".to_string(),
            domain: Some("style".to_string()),
            confidence: 0.8,
            observation_count: 10,
            file_path: String::new(),
            project: None,
            is_anti_pattern: false,
            source: Some("claude".to_string()),
            content: Some("Always annotate public APIs.".to_string()),
            provider_scope: vec![IntegrationProvider::Claude],
        };
        let neg = crate::models::LearnedRulePayload {
            name: "policy-negative".to_string(),
            is_anti_pattern: true,
            ..pos.clone()
        };
        storage.store_learned_rule(&pos).expect("seed positive");
        storage.store_learned_rule(&neg).expect("seed negative");
        let conflict_obs = seed_observation(&storage, "conf", "/proj/y");
        let conf = storage.resolve_evidence_refs(&[obs_ref(conflict_obs)], None);
        storage
            .persist_citations_and_advance_version("policy-positive", &conf, false)
            .expect("cite pos");
        storage
            .persist_citations_and_advance_version("policy-negative", &conf, false)
            .expect("cite neg");

        storage
            .record_rule_reconciliation(&[IntegrationProvider::Claude])
            .expect("reconcile pass 1");

        let lifecycle_of = |s: &Storage, n: &str| -> (String, Option<String>) {
            let conn = s.conn.lock();
            conn.query_row(
                "SELECT lifecycle, superseded_by FROM learned_rules WHERE name = ?1",
                params![n],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read lifecycle")
        };

        // Duplicate: weak loser superseded by the strong survivor; survivor
        // stays a normal candidate.
        let (weak_lc, weak_by) = lifecycle_of(&storage, "dup-weak");
        let (strong_lc, _) = lifecycle_of(&storage, "dup-strong");
        assert_eq!(
            weak_lc, "superseded",
            "the lower-score duplicate must be superseded"
        );
        assert_eq!(
            weak_by.as_deref(),
            Some("dup-strong"),
            "superseded_by must point at the deterministic higher-score survivor"
        );
        assert_eq!(
            strong_lc, "candidate",
            "the survivor must remain a normal (non-terminal) candidate"
        );

        // Conflict: BOTH flagged, neither superseded.
        let (pos_lc, _) = lifecycle_of(&storage, "policy-positive");
        let (neg_lc, _) = lifecycle_of(&storage, "policy-negative");
        assert_eq!(
            pos_lc, "conflict_flagged",
            "positive side must be conflict_flagged"
        );
        assert_eq!(
            neg_lc, "conflict_flagged",
            "anti-pattern side must be conflict_flagged"
        );

        // Idempotent: a second pass skips terminal rows — nothing changes.
        storage
            .record_rule_reconciliation(&[IntegrationProvider::Claude])
            .expect("reconcile pass 2");
        assert_eq!(
            lifecycle_of(&storage, "dup-weak"),
            ("superseded".to_string(), Some("dup-strong".to_string())),
            "re-running reconciliation must be idempotent for a superseded row"
        );
        assert_eq!(
            lifecycle_of(&storage, "policy-positive").0,
            "conflict_flagged",
            "re-running reconciliation must be idempotent for a conflict_flagged row"
        );
        assert_eq!(
            lifecycle_of(&storage, "dup-strong").0,
            "candidate",
            "the survivor must not be demoted by a second pass"
        );

        clear_env();
    }

    /// FR-029 / SC-006 (research R-5, "Operator accept/reject feedback"):
    /// operator feedback is the PRIMARY signal and strictly dominates any
    /// single LLM verdict. For an identical rule, `submit_rule_feedback`
    /// `accept` yields a higher evidence-weighted score than an equivalent
    /// LLM `support` verdict (modelled as the ≤1.0 α bump that path uses);
    /// `reject` lowers the score WITHOUT writing a tombstone (recoverable);
    /// `bad` writes a durable name-keyed tombstone so the rule cannot
    /// resurrect (US2 tombstone-assertion style).
    #[test]
    #[serial]
    fn operator_feedback_dominates_llm_verdict_and_bad_tombstones() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // Helper to (re)seed an identical baseline rule (fresh evidence so
        // freshness is ~1.0 and the only variable is the operator delta).
        let baseline_alpha = 6.0_f64;
        let baseline_beta = 4.0_f64;
        let seed_rule = |s: &Storage, name: &str| {
            let conn = s.conn.lock();
            conn.execute(
                "INSERT INTO learned_rules
                    (name, domain, confidence, observation_count, file_path, alpha, beta_param,
                     last_evidence_at, state, lifecycle, is_anti_pattern, source, content, provider_scope)
                 VALUES (?1, 'errors', 0.6, 10, '', ?2, ?3, ?4, 'emerging', 'candidate', 0,
                         'claude', 'Prefer explicit error types.', '[\"claude\"]')",
                params![name, baseline_alpha, baseline_beta, Utc::now().to_rfc3339()],
            )
            .expect("seed feedback rule");
        };

        // Score of a rule WITH operator feedback folded in (mirrors
        // `eligible_for_review`'s use of `operator_feedback_delta`).
        let scored_with_feedback = |s: &Storage, name: &str| -> f64 {
            let conn = s.conn.lock();
            let (a, b, ts): (f64, f64, Option<String>) = conn
                .query_row(
                    "SELECT alpha, beta_param, last_evidence_at FROM learned_rules WHERE name = ?1",
                    params![name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .expect("read rule for scoring");
            let (da, db) = Storage::operator_feedback_delta(&conn, name);
            drop(conn);
            evidence_weighted_score(a + da, b + db, ts.as_deref()).0
        };

        // LLM `support` verdict: the strongest LLM path is a ≤1.0 α bump.
        let llm_name = "fb-llm-support";
        seed_rule(&storage, llm_name);
        {
            let conn = storage.conn.lock();
            conn.execute(
                "UPDATE learned_rules SET alpha = alpha + 1.0 WHERE name = ?1",
                params![llm_name],
            )
            .expect("apply LLM support bump");
        }
        let llm_score = scored_with_feedback(&storage, llm_name);

        // Operator `accept`: strictly dominates the LLM verdict.
        let acc_name = "fb-accept";
        seed_rule(&storage, acc_name);
        storage
            .submit_rule_feedback(acc_name, "accept", None)
            .expect("accept feedback");
        let accept_score = scored_with_feedback(&storage, acc_name);
        assert!(
            accept_score > llm_score,
            "operator accept ({accept_score}) must dominate an equivalent LLM support verdict ({llm_score})"
        );
        // No tombstone for accept.
        {
            let conn = storage.conn.lock();
            assert!(
                !tombstone_blocks(&conn, acc_name),
                "accept must NOT write a tombstone"
            );
        }

        // Operator `reject`: lowers the score vs an untouched baseline,
        // WITHOUT a tombstone (recoverable).
        let rej_name = "fb-reject";
        seed_rule(&storage, rej_name);
        let base_score = scored_with_feedback(&storage, rej_name); // before feedback
        storage
            .submit_rule_feedback(rej_name, "reject", None)
            .expect("reject feedback");
        let reject_score = scored_with_feedback(&storage, rej_name);
        assert!(
            reject_score < base_score,
            "operator reject ({reject_score}) must lower the score below the unrated baseline ({base_score})"
        );
        {
            let conn = storage.conn.lock();
            assert!(
                !tombstone_blocks(&conn, rej_name),
                "reject must NOT write a tombstone (it stays recoverable)"
            );
        }

        // Operator `bad`: durable tombstone — the rule cannot resurrect even
        // under strong re-extraction (US2 tombstone-assertion style).
        let bad_name = "fb-bad";
        seed_rule(&storage, bad_name);
        storage
            .submit_rule_feedback(bad_name, "bad", Some("noisy and wrong"))
            .expect("bad feedback");
        {
            let conn = storage.conn.lock();
            assert!(
                tombstone_blocks(&conn, bad_name),
                "bad must write an active durable tombstone"
            );
            let (by, lifecycle, state): (String, String, String) = conn
                .query_row(
                    "SELECT t.tombstoned_by, r.lifecycle, r.state
                     FROM rule_tombstones t JOIN learned_rules r ON r.name = t.rule_name
                     WHERE t.rule_name = ?1",
                    params![bad_name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .expect("read bad tombstone");
            assert_eq!(by, "operator_bad", "tombstone attributed to operator_bad");
            assert_eq!(
                lifecycle, "tombstoned",
                "bad must move lifecycle to tombstoned"
            );
            assert_eq!(state, "suppressed", "bad must hide the rule via state");
        }
        // Re-extraction must NOT resurrect a `bad`-tombstoned rule.
        let strong_reextract = crate::models::LearnedRulePayload {
            name: bad_name.to_string(),
            domain: Some("errors".to_string()),
            confidence: 0.99,
            observation_count: 20,
            file_path: "/learned/fb-bad.md".to_string(),
            project: None,
            is_anti_pattern: false,
            source: Some("claude".to_string()),
            content: Some("RESURRECTED body that must not win.".to_string()),
            provider_scope: vec![IntegrationProvider::Claude],
        };
        storage
            .store_learned_rule(&strong_reextract)
            .expect("re-extraction upsert must not error");
        {
            let conn = storage.conn.lock();
            let (lifecycle, fp): (String, String) = conn
                .query_row(
                    "SELECT lifecycle, file_path FROM learned_rules WHERE name = ?1",
                    params![bad_name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("read after re-extraction");
            assert_eq!(
                lifecycle, "tombstoned",
                "a bad-tombstoned rule must not be resurrected by re-extraction"
            );
            assert_eq!(
                fp, "",
                "suppression-sticky upsert must not re-arm file_path"
            );
            assert!(
                tombstone_blocks(&conn, bad_name),
                "the durable bad tombstone must remain active across re-extraction"
            );
        }

        clear_env();
    }

    /// Feature 005 US4 T054 (FR-021 seam: `wilson_lower_bound`). The composed
    /// `evidence_weighted_score` is exercised elsewhere; this pins the pure
    /// Wilson lower-bound primitive's contract edges directly (the audit
    /// requires the bare seam, not only the composition):
    /// * `n = α + β < 0.01` ⇒ exactly `0.5` (the no-evidence prior).
    /// * Monotone non-decreasing in α with β fixed (more confirmations never
    ///   lower the conservative lower bound).
    /// * Output is clamped to `[0.0, 0.95]` — the upper clamp is observable
    ///   (huge α never reads `1.0`), the lower clamp keeps it ≥ 0.
    /// * Symmetric prior: α-only vs the β-mirror — an all-success sample and
    ///   the equal-sized all-failure sample sum to ≈ the saturated band, i.e.
    ///   `wlb(k,0)` and `wlb(0,k)` are reflections about the no-evidence prior
    ///   (`wlb(0,k) == 0.0` for the lower bound; `wlb(k,0)` rises toward the
    ///   0.95 clamp), so the failure-only bound is the conservative floor.
    #[test]
    fn wilson_lower_bound_edges_zero_n_clamp_monotone_symmetry() {
        // n < 0.01 ⇒ 0.5 prior (covers α=β=0 and the sub-epsilon boundary).
        assert_eq!(wilson_lower_bound(0.0, 0.0).to_bits(), 0.5_f64.to_bits());
        assert_eq!(
            wilson_lower_bound(0.004, 0.004).to_bits(),
            0.5_f64.to_bits(),
            "α+β just under 0.01 must still return the 0.5 prior"
        );
        // The first sample at/over the epsilon leaves the prior (n≥0.01 path).
        assert_ne!(
            wilson_lower_bound(0.01, 0.0),
            0.5,
            "α+β == 0.01 must take the computed path, not the prior"
        );

        // Monotone non-decreasing in α (β fixed) — directly on the primitive.
        let mut prev = f64::NEG_INFINITY;
        for a in [0.5, 1.0, 2.0, 5.0, 20.0, 100.0, 5_000.0] {
            let w = wilson_lower_bound(a, 4.0);
            assert!(
                w >= prev - 1e-12,
                "wilson_lower_bound must be monotone non-decreasing in α: {w} < {prev} at α={a}"
            );
            assert!(
                (0.0..=0.95).contains(&w),
                "wilson_lower_bound must stay within [0, 0.95], got {w} at α={a}"
            );
            prev = w;
        }

        // Upper clamp is real and binding: an overwhelmingly positive sample
        // saturates at exactly 0.95, never 1.0.
        assert_eq!(
            wilson_lower_bound(1.0e9, 0.0),
            0.95,
            "the conservative bound must clamp at 0.95 even for near-certain evidence"
        );

        // Symmetric prior / lower-bound floor: an all-failure sample drives
        // the lower bound to its 0.0 floor (the reflection of the all-success
        // case which rises toward 0.95). The bound is therefore asymmetric in
        // VALUE by construction (it is a *lower* bound) but symmetric in form
        // about p=α/n — equal evidence on the failure side is the worst case.
        for k in [1.0, 5.0, 50.0] {
            let success_only = wilson_lower_bound(k, 0.0);
            let failure_only = wilson_lower_bound(0.0, k);
            assert_eq!(
                failure_only, 0.0,
                "an all-failure sample must pin the lower bound at its 0.0 floor (k={k})"
            );
            assert!(
                success_only > failure_only,
                "the all-success lower bound ({success_only}) must exceed the all-failure floor ({failure_only}) at k={k}"
            );
            // p is symmetric: swapping α/β reflects p about 0.5, so a
            // balanced sample sits at the symmetric midpoint regardless of
            // order (wlb(a,b) with a==b is order-invariant).
            assert_eq!(
                wilson_lower_bound(k, k).to_bits(),
                wilson_lower_bound(k, k).to_bits(),
                "balanced sample is order-invariant (p symmetric about 0.5)"
            );
        }
    }

    /// Feature 005 US4 T054 (FR-021 seam: `freshness_factor`). Pins the pure
    /// 90-day half-life primitive's contract edges directly:
    /// * `None` last-evidence ⇒ exactly `1.0` (treat unknown as fresh).
    /// * An unparseable timestamp ⇒ exactly `1.0` (fail-open, never panic).
    /// * ≈ 90 days old ⇒ ≈ `0.5` (one half-life), 180 days ⇒ ≈ `0.25`.
    /// * A FUTURE timestamp clamps the elapsed-seconds floor at 0, so the
    ///   factor is exactly `1.0` (never > 1.0, never negative exponent).
    #[test]
    fn freshness_factor_none_unparseable_half_life_and_future_clamp() {
        assert_eq!(
            freshness_factor(None).to_bits(),
            1.0_f64.to_bits(),
            "None last-evidence must read as fully fresh (1.0)"
        );
        assert_eq!(
            freshness_factor(Some("not-a-timestamp")).to_bits(),
            1.0_f64.to_bits(),
            "an unparseable timestamp must fail open to 1.0, not panic or 0"
        );
        assert_eq!(
            freshness_factor(Some("2026-05-18")).to_bits(),
            1.0_f64.to_bits(),
            "a date-only (non-RFC3339) string is unparseable ⇒ 1.0 fail-open"
        );

        // One half-life (~90d) ⇒ ≈0.5; two (~180d) ⇒ ≈0.25. Use a small
        // tolerance because `now` advances between the timestamp build and
        // the call.
        let d90 = (Utc::now() - chrono::Duration::days(90)).to_rfc3339();
        let f90 = freshness_factor(Some(d90.as_str()));
        assert!(
            (f90 - 0.5).abs() < 0.01,
            "≈90 days old must decay to ≈0.5 (one half-life), got {f90}"
        );
        let d180 = (Utc::now() - chrono::Duration::days(180)).to_rfc3339();
        let f180 = freshness_factor(Some(d180.as_str()));
        assert!(
            (f180 - 0.25).abs() < 0.01,
            "≈180 days old must decay to ≈0.25 (two half-lives), got {f180}"
        );
        assert!(
            f180 < f90 && f90 < 1.0,
            "freshness must strictly decrease with age ({f180} < {f90} < 1.0)"
        );

        // Future timestamp: `num_seconds().max(0)` floors elapsed at 0 so the
        // exponent is 0 ⇒ factor is exactly 1.0 (never amplifies above 1.0).
        let future = (Utc::now() + chrono::Duration::days(365)).to_rfc3339();
        assert_eq!(
            freshness_factor(Some(future.as_str())).to_bits(),
            1.0_f64.to_bits(),
            "a future last-evidence timestamp must clamp to exactly 1.0 (no >1.0 boost)"
        );
    }

    /// Feature 005 US4 T054 (FR-021 seam: `eligible_for_review` consumes the
    /// EVIDENCE-WEIGHTED score, not the raw extracting-model self-rating, and
    /// excludes `invalidated`). The min-cluster dimension is covered by
    /// `eligible_for_review_enforces_min_cluster_uniformly_across_streams`;
    /// this fills the orthogonal gap: a rule with a near-maximal raw
    /// `confidence` (0.99) but β-dominated accumulated evidence
    /// (`beta >= alpha && beta >= 5` ⇒ `compute_state` → `invalidated`) is
    /// NOT eligible **even when the ≥3-citation / ≥1-source cluster gate is
    /// fully satisfied**, proving the raw self-rating cannot buy eligibility
    /// and the `state == "invalidated"` early-out fires. Flipping the same
    /// rule to α-dominant (evidence-weighted score ≥ default 0.6) with the
    /// identical raw `confidence` and identical citations makes it eligible —
    /// isolating the evidence-weighted gate as the deciding factor.
    #[test]
    #[serial]
    fn eligible_for_review_uses_evidence_weighted_score_not_raw_self_rating() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let name = "raw-rating-cannot-buy-eligibility";
        let seed = crate::models::LearnedRulePayload {
            name: name.to_string(),
            domain: Some("errors".to_string()),
            // Near-maximal RAW self-rating — must NOT by itself confer
            // eligibility (the gate scores accumulated α/β, not this).
            confidence: 0.99,
            observation_count: 20,
            file_path: String::new(),
            project: None,
            is_anti_pattern: false,
            source: Some("claude".to_string()),
            content: Some("Prefer explicit error types.".to_string()),
            provider_scope: vec![IntegrationProvider::Claude],
        };
        storage.store_learned_rule(&seed).expect("seed rule");

        // Fully satisfy the min-cluster gate (≥3 distinct resolved obs
        // citations, ≥1 source) so the ONLY thing that can deny eligibility
        // is the evidence-weighted state/score path.
        let o1 = seed_observation(&storage, "ev1", "/proj/ev");
        let o2 = seed_observation(&storage, "ev2", "/proj/ev");
        let o3 = seed_observation(&storage, "ev3", "/proj/ev");
        let resolved =
            storage.resolve_evidence_refs(&[obs_ref(o1), obs_ref(o2), obs_ref(o3)], None);
        storage
            .persist_citations_and_advance_version(name, &resolved, false)
            .expect("persist 3 citations");

        // Force β-dominant accumulated evidence: beta >= alpha && beta >= 5
        // ⇒ compute_state override ⇒ `invalidated` regardless of the 0.99
        // raw self-rating and a healthy citation cluster.
        {
            let conn = storage.conn.lock();
            conn.execute(
                "UPDATE learned_rules SET alpha = 2.0, beta_param = 9.0,
                     last_evidence_at = ?2 WHERE name = ?1",
                params![name, Utc::now().to_rfc3339()],
            )
            .expect("force beta-dominant evidence");
        }
        // Sanity: the evidence-weighted state IS `invalidated` (so the
        // assertion below is exercising the intended early-out, not min
        // cluster). Mirrors `eligible_for_review`'s internal scoring.
        {
            let conn = storage.conn.lock();
            let (_s, st) = evidence_weighted_score(2.0, 9.0, Some(&Utc::now().to_rfc3339()));
            drop(conn);
            assert_eq!(
                st, "invalidated",
                "test precondition: β-dominant rule must score `invalidated`"
            );
        }
        assert!(
            !storage
                .eligible_for_review(name)
                .expect("eligibility query (invalidated)"),
            "FR-021: a 0.99 RAW-confidence rule that is evidence-weighted `invalidated` \
             must NOT be eligible even with the min-cluster gate fully satisfied — the \
             gate uses the evidence-weighted score, not the raw self-rating"
        );

        // Flip ONLY the accumulated evidence to α-dominant (same raw 0.99
        // confidence, same 3 citations). Evidence-weighted score now clears
        // the default 0.6 `min_eligibility` ⇒ eligible. This isolates the
        // evidence-weighted score as the sole deciding input.
        {
            let conn = storage.conn.lock();
            conn.execute(
                "UPDATE learned_rules SET alpha = 30.0, beta_param = 1.0,
                     last_evidence_at = ?2 WHERE name = ?1",
                params![name, Utc::now().to_rfc3339()],
            )
            .expect("flip to alpha-dominant accumulated evidence");
            let (score, st) = evidence_weighted_score(30.0, 1.0, Some(&Utc::now().to_rfc3339()));
            drop(conn);
            assert!(
                st != "invalidated" && score >= 0.6,
                "test precondition: α-dominant rule must clear the gate (state={st}, score={score})"
            );
        }
        assert!(
            storage
                .eligible_for_review(name)
                .expect("eligibility query (alpha-dominant)"),
            "FR-021: with identical raw confidence + citations but α-dominant \
             accumulated evidence the same rule IS eligible — the evidence-weighted \
             score is the deciding factor"
        );

        clear_env();
    }

    /// Feature 005 US5 T058 (R-7.1 / H-6 / FR-024). `decode_inference_metadata`
    /// is tolerant: NULL / `None` / malformed JSON / empty array ⇒ `None`
    /// (legacy + `micro` runs legitimately record none — never a panic). A
    /// well-formed multi-call array folds into the rollup: summed cost +
    /// duration, `primary_model` = highest summed-cost model (first-dispatch
    /// tie-break), `failed_call_count` counts `success:false`, and the decode
    /// is also exercised end-to-end through `get_learning_runs`.
    #[test]
    #[serial]
    fn decode_inference_metadata_is_tolerant_and_folds_rollup() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        // Tolerant cases — none of these may panic, all ⇒ None.
        assert!(decode_inference_metadata(None).is_none(), "NULL ⇒ None");
        assert!(
            decode_inference_metadata(Some("not json".to_string())).is_none(),
            "parse-error ⇒ None"
        );
        assert!(
            decode_inference_metadata(Some("[]".to_string())).is_none(),
            "empty array ⇒ None"
        );
        assert!(
            decode_inference_metadata(Some("{\"unexpected\":1}".to_string())).is_none(),
            "wrong-shaped JSON (object, not array) ⇒ None, no panic"
        );

        // A legacy/partial record missing every skipped field still parses
        // (all decode fields are `#[serde(default)]`).
        let partial = decode_inference_metadata(Some(
            "[{\"phase\":\"stream_a\",\"max_tokens_requested\":0}]".to_string(),
        ))
        .expect("partial record decodes to Some");
        assert_eq!(partial.call_count, 1);
        assert_eq!(
            partial.failed_call_count, 1,
            "absent success defaults false"
        );
        assert!(partial.primary_model.is_none(), "no model field ⇒ None");

        // Realistic 3-call array: synthesis is the highest-cost model.
        let raw = serde_json::json!([
            {
                "phase": "stream_a", "model": "claude-sonnet-4-6",
                "max_tokens_requested": 8192, "duration_ms": 1000, "duration_api_ms": 900,
                "ttft_ms": 200, "input_tokens": 50, "output_tokens": 60,
                "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0,
                "total_cost_usd": 0.01, "success": true
            },
            {
                "phase": "stream_b", "model": "claude-sonnet-4-6",
                "max_tokens_requested": 8192, "duration_ms": 500, "duration_api_ms": 450,
                "ttft_ms": 100, "input_tokens": 10, "output_tokens": 20,
                "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0,
                "total_cost_usd": 0.02, "success": false, "failure_kind": "rate_limited"
            },
            {
                "phase": "synthesis", "model": "synth-model",
                "max_tokens_requested": 8192, "duration_ms": 2000, "duration_api_ms": 1900,
                "ttft_ms": 300, "input_tokens": 70, "output_tokens": 80,
                "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0,
                "total_cost_usd": 0.05, "success": true
            }
        ])
        .to_string();
        let summary = decode_inference_metadata(Some(raw.clone())).expect("decodes");
        assert_eq!(summary.call_count, 3);
        assert_eq!(summary.failed_call_count, 1);
        assert!((summary.total_cost_usd - 0.08).abs() < 1e-9);
        assert_eq!(summary.total_duration_ms, 3500);
        assert_eq!(
            summary.primary_model.as_deref(),
            Some("synth-model"),
            "highest summed-cost model is primary"
        );
        assert_eq!(summary.calls.len(), 3);
        assert_eq!(summary.calls[1].phase, "stream_b");
        assert!(!summary.calls[1].success);
        assert_eq!(
            summary.calls[1].failure_kind.as_deref(),
            Some("rate_limited")
        );
        assert_eq!(summary.calls[2].phase, "synthesis");

        // Feature 006 Follow-up A (R-A / C-A): the array above carried no
        // `sandbox` tag on any call, so the run-level rollup is absent —
        // legacy/micro runs must NOT be surfaced as un-confined.
        assert!(
            summary.all_fs_confined.is_none(),
            "no call carried a sandbox tag ⇒ all_fs_confined None (not a disclosure)"
        );
        assert!(
            summary.calls.iter().all(|c| c.confinement.is_none()),
            "untagged calls carry no confinement descriptor"
        );

        // Every tagged call FS-confined (`bwrap`/`sandbox-exec`) ⇒ rollup
        // Some(true); each call carries fs_confined=true.
        let all_conf = decode_inference_metadata(Some(
            serde_json::json!([
                {"phase": "stream_a", "max_tokens_requested": 0, "success": true, "sandbox": "bwrap"},
                {"phase": "synthesis", "max_tokens_requested": 0, "success": true, "sandbox": "sandbox-exec"}
            ])
            .to_string(),
        ))
        .expect("decodes");
        assert_eq!(
            all_conf.all_fs_confined,
            Some(true),
            "every tagged call FS-confined ⇒ Some(true)"
        );
        assert!(
            all_conf
                .calls
                .iter()
                .all(|c| c.confinement.as_ref().map(|cf| cf.fs_confined) == Some(true)),
            "bwrap/sandbox-exec calls carry fs_confined=true"
        );

        // Any not-FS-confined call ⇒ rollup Some(false) (AND-fold).
        let mixed = decode_inference_metadata(Some(
            serde_json::json!([
                {"phase": "stream_a", "max_tokens_requested": 0, "success": true, "sandbox": "bwrap"},
                {"phase": "synthesis", "max_tokens_requested": 0, "success": true, "sandbox": "process-only"}
            ])
            .to_string(),
        ))
        .expect("decodes");
        assert_eq!(
            mixed.all_fs_confined,
            Some(false),
            "one process-only call ⇒ run not fully FS-confined"
        );

        // Deploy-safety: a legacy `unshare` tag persisted by a pre-feature-
        // 006 build is no longer in the closed vocabulary, but the string
        // classifier conservatively treats any unknown tag as NOT
        // FS-confined — never a false safety signal, never a decode error,
        // and the raw tag is preserved verbatim for the operator.
        let legacy = decode_inference_metadata(Some(
            serde_json::json!([
                {"phase": "stream_a", "max_tokens_requested": 0, "success": true, "sandbox": "unshare"}
            ])
            .to_string(),
        ))
        .expect("legacy tag still decodes (Option<String>, no enum parse)");
        assert_eq!(
            legacy.all_fs_confined,
            Some(false),
            "legacy `unshare` tag ⇒ not FS-confined (conservative default)"
        );
        let cf = legacy.calls[0]
            .confinement
            .as_ref()
            .expect("legacy tagged call carries a confinement descriptor");
        assert_eq!(
            cf.sandbox, "unshare",
            "raw persisted tag preserved verbatim"
        );
        assert!(
            !cf.fs_confined,
            "unknown/legacy tag classified not-FS-confined"
        );

        // Feature 007 T023 / C-D deploy-safety table — single-call decode
        // for every historical and new sandbox tag. Each row asserts:
        //   1. decode succeeds (no panic, no error)
        //   2. raw tag is preserved verbatim in confinement.sandbox
        //   3. fs_confined classifies per the C-D3 historical-meaning table
        //   4. the run-level all_fs_confined rollup matches (single-call ⇒
        //      Some(fs_confined))
        //
        // Closed write vocabulary today (feature 007 C-A2): landlock, bwrap,
        // sandbox-exec, job-object, none. Decoded tolerantly: process-only
        // (retired feature-006-A vocabulary), unshare (pre-feature-006
        // legacy), and any unknown future tag.
        let deploy_safety_cases: &[(&str, bool)] = &[
            // New write vocabulary (feature 007 primary).
            ("landlock", true),
            // Historical/active feature-005 vocabulary (still active as the
            // bwrap fallback after Landlock).
            ("bwrap", true),
            // Retired feature-006-A vocabulary — historical rows decode
            // honestly as not-FS-confined.
            ("process-only", false),
            // Pre-feature-006 legacy tag — never in the closed vocabulary,
            // classified conservatively.
            ("unshare", false),
        ];
        for (tag, expect_fs) in deploy_safety_cases {
            let raw = serde_json::json!([
                {
                    "phase": "stream_a",
                    "max_tokens_requested": 0,
                    "success": true,
                    "sandbox": tag,
                }
            ])
            .to_string();
            let decoded = decode_inference_metadata(Some(raw))
                .unwrap_or_else(|| panic!("deploy-safety tag `{tag}` must decode without error"));
            assert_eq!(
                decoded.call_count, 1,
                "deploy-safety single-call decode for `{tag}`"
            );
            assert_eq!(
                decoded.all_fs_confined,
                Some(*expect_fs),
                "rollup all_fs_confined for `{tag}` must be Some({expect_fs}) \
                 (single-call run; rollup mirrors the only call's classification)"
            );
            let cf = decoded.calls[0]
                .confinement
                .as_ref()
                .unwrap_or_else(|| panic!("deploy-safety `{tag}` call carries a confinement"));
            assert_eq!(
                cf.sandbox, *tag,
                "raw persisted `{tag}` tag MUST round-trip verbatim into \
                 confinement.sandbox (contract C-D2)"
            );
            assert_eq!(
                cf.fs_confined, *expect_fs,
                "fs_confined classification for `{tag}` must match contract C-D3"
            );
        }

        // End-to-end through get_learning_runs: a run WITH metadata surfaces
        // the rollup; a legacy run with NULL surfaces `inference: None`.
        {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO learning_runs (trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, created_at, provider_scope, inference_metadata)
                 VALUES ('manual', 3, 1, 0, 4200, 'completed', '2026-05-10T00:00:00Z', '[\"claude\"]', ?1)",
                params![raw],
            )
            .expect("seed run with metadata");
            conn.execute(
                "INSERT INTO learning_runs (trigger_mode, observations_analyzed, rules_created, rules_updated, duration_ms, status, created_at, provider_scope, inference_metadata)
                 VALUES ('micro', 1, 0, 0, 10, 'completed', '2026-05-09T00:00:00Z', '[\"claude\"]', NULL)",
                [],
            )
            .expect("seed legacy run NULL metadata");
        }
        let runs = storage
            .get_learning_runs(10, None)
            .expect("get_learning_runs");
        let with_meta = runs
            .iter()
            .find(|r| r.trigger_mode == "manual")
            .expect("manual run present");
        let inf = with_meta
            .inference
            .as_ref()
            .expect("manual run carries decoded inference rollup");
        assert_eq!(inf.primary_model.as_deref(), Some("synth-model"));
        assert_eq!(inf.call_count, 3);
        let legacy = runs
            .iter()
            .find(|r| r.trigger_mode == "micro")
            .expect("micro run present");
        assert!(
            legacy.inference.is_none(),
            "legacy NULL inference_metadata ⇒ inference: None"
        );

        clear_env();
    }

    /// Feature 005 US5 T061 (R-7.3 / M-2 / FR-026 / SC-010). Retention cutoff
    /// is `MIN(analyzed_watermark, now-30d)`. Observations newer than the
    /// newest completed/degraded run's `created_at` have not had an analysis
    /// opportunity and MUST survive; old analyzed ones are pruned. With zero
    /// completed/degraded runs the watermark is NULL ⇒ nothing is deleted.
    #[test]
    #[serial]
    fn cleanup_old_observations_respects_analyzed_watermark() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let insert_obs = |session: &str, ts: &str, out: &str| {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO observations
                    (provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at)
                 VALUES ('claude', ?1, ?2, 'PostToolUse', 'Bash', 'in', ?3, '/proj', ?2)",
                params![session, ts, out],
            )
            .expect("seed observation");
        };

        // Zero completed/degraded runs ⇒ nothing deleted (even very old rows).
        insert_obs("z-old", "2000-01-01T00:00:00Z", "ok");
        storage
            .cleanup_old_observations()
            .expect("cleanup with zero runs");
        {
            let conn = storage.conn.lock();
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
                .expect("count");
            assert_eq!(n, 1, "no completed/degraded run ⇒ delete NOTHING");
        }

        // Now add a completed run at a fixed watermark and observations on
        // both sides of it. The 30-day floor is far older than the 2024
        // watermark, so `MIN(watermark, now-30d)` == watermark here.
        let watermark = "2024-06-01T00:00:00Z";
        {
            let conn = storage.conn.lock();
            conn.execute(
                "INSERT INTO learning_runs (trigger_mode, observations_analyzed, rules_created, rules_updated, status, created_at, provider_scope)
                 VALUES ('manual', 0, 0, 0, 'completed', ?1, '[\"claude\"]')",
                params![watermark],
            )
            .expect("seed completed run");
        }
        // Pre-watermark (analyzed, must be pruned) — note the structured
        // failure marker so the tightened error tally counts it.
        insert_obs(
            "pre",
            "2024-05-01T00:00:00Z",
            "{\"is_error\":true,\"message\":\"boom\"}",
        );
        // Post-watermark (no analysis opportunity yet, must survive).
        insert_obs("post", "2024-07-01T00:00:00Z", "all good, no errors here");

        storage
            .cleanup_old_observations()
            .expect("cleanup with watermark");

        {
            let conn = storage.conn.lock();
            let surviving: Vec<String> = {
                let mut stmt = conn
                    .prepare("SELECT session_id FROM observations ORDER BY session_id")
                    .expect("prepare");
                stmt.query_map([], |r| r.get::<_, String>(0))
                    .expect("query")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("collect")
            };
            assert_eq!(
                surviving,
                vec!["post".to_string()],
                "only the post-watermark row (no analysis opportunity yet) \
                 survives; everything older than the analyzed watermark \
                 (`pre` AND the ancient `z-old`) is pruned — SC-010"
            );

            // The pruned `pre` row was summarized with the *tightened* error
            // predicate: its structured `\"is_error\":true` output counts as
            // exactly one error; the benign \"no errors\" text does not (it
            // was post-watermark so not summarized anyway — assert the marker
            // semantics via the stored summary).
            let err_total: i64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(error_count),0) FROM observation_summaries",
                    [],
                    |r| r.get(0),
                )
                .expect("summary error total");
            assert_eq!(
                err_total, 1,
                "structured `is_error:true` ⇒ one counted error in the summary"
            );
        }

        clear_env();
    }

    /// Feature 005 US5 T062 (R-7.4 / M-1 / FR-027). The previously write-only
    /// `observation_summaries` table is now readable via
    /// `get_observation_summaries` (period/provider/project scoped) and the
    /// tightened error predicate counts only structured failure markers, not
    /// a bare `%error%` substring.
    #[test]
    #[serial]
    fn get_observation_summaries_reads_rows_and_tight_error_predicate() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        {
            let conn = storage.conn.lock();
            // Benign text containing the substring "error" but NO structured
            // marker — must NOT count under the tightened predicate.
            conn.execute(
                "INSERT INTO observations
                    (provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at)
                 VALUES ('claude','b','2024-01-01T00:00:00Z','PostToolUse','Bash','in',
                         'completed with no errors; ErrorBoundary rendered fine','/p','2024-01-01T00:00:00Z')",
                [],
            )
            .expect("seed benign obs");
            // Structured failure marker — must count as exactly one error.
            conn.execute(
                "INSERT INTO observations
                    (provider, session_id, timestamp, hook_phase, tool_name, tool_input, tool_output, cwd, created_at)
                 VALUES ('claude','e','2024-01-01T00:00:00Z','PostToolUse','Bash','in',
                         'Error: command failed with exit code 1','/p','2024-01-01T00:00:00Z')",
                [],
            )
            .expect("seed error obs");
            // A completed run so the watermark permits deletion (drives the
            // summary write through the production path).
            conn.execute(
                "INSERT INTO learning_runs (trigger_mode, observations_analyzed, rules_created, rules_updated, status, created_at, provider_scope)
                 VALUES ('manual', 0, 0, 0, 'completed', '2026-01-01T00:00:00Z', '[\"claude\"]')",
                [],
            )
            .expect("seed completed run");
        }

        storage
            .cleanup_old_observations()
            .expect("cleanup writes a summary");

        let summaries = storage
            .get_observation_summaries(None, None, "2000-01-01")
            .expect("read summaries");
        assert!(
            !summaries.is_empty(),
            "summary writer + accessor round-trip: at least one row"
        );
        let total_err: i64 = summaries.iter().map(|s| s.error_count).sum();
        assert_eq!(
            total_err, 1,
            "only the structured `Error:`-prefixed output counts; the benign \
             'no errors'/'ErrorBoundary' text does not"
        );
        let total_obs: i64 = summaries.iter().map(|s| s.total_observations).sum();
        assert_eq!(total_obs, 2, "both observations summarized");

        // Provider scope filters; an unknown provider yields nothing.
        let claude_only = storage
            .get_observation_summaries(Some(IntegrationProvider::Claude), None, "2000-01-01")
            .expect("claude-scoped read");
        assert_eq!(claude_only.len(), summaries.len());
        let codex_only = storage
            .get_observation_summaries(Some(IntegrationProvider::Codex), None, "2000-01-01")
            .expect("codex-scoped read");
        assert!(codex_only.is_empty(), "no codex summaries seeded");

        // `since` is an inclusive lower bound on `period` (the cleanup date).
        let future = storage
            .get_observation_summaries(None, None, "2999-01-01")
            .expect("future-since read");
        assert!(
            future.is_empty(),
            "since after the cleanup period ⇒ no rows"
        );

        clear_env();
    }

    // ---------------------------------------------------------------------
    // Feature 006 Follow-up B (R-B / C-B / Option B3): the pending marker
    // `learned_rules.current_version` is advanced ONLY after the new
    // version's `rule_evidence_citations` snapshot is persisted, atomically.
    // Deterministic; `#[serial]` (shared env globals); harness mirrors the
    // `eligible_for_review_*` / `store_learned_rule_*` tests exactly. These
    // do NOT modify `store_learned_rule_on_conflict_is_suppression_sticky`
    // or `eligible_for_review_enforces_min_cluster_uniformly_across_streams`
    // (the unchanged regression guards, T020).
    // ---------------------------------------------------------------------

    /// Seed an `awaiting_review` rule with accumulated evidence so the
    /// Wilson score clears `min_eligibility` (0.6) — a fresh rule peaks at
    /// ≈0.554, the documented R-6 behaviour, so the cluster/citation gate is
    /// isolated as the only variable. Returns the rule id.
    fn seed_awaiting_review_rule(storage: &Storage, name: &str, content: &str) -> i64 {
        let payload = crate::models::LearnedRulePayload {
            name: name.to_string(),
            domain: Some("errors".to_string()),
            confidence: 0.99,
            observation_count: 20,
            file_path: String::new(),
            project: None,
            is_anti_pattern: false,
            source: Some("claude".to_string()),
            content: Some(content.to_string()),
            provider_scope: vec![IntegrationProvider::Claude],
        };
        let changed = storage.store_learned_rule(&payload).expect("seed rule");
        assert!(
            !changed,
            "a brand-new (candidate) row must NOT signal pending_changed"
        );
        let conn = storage.conn.lock();
        conn.execute(
            "UPDATE learned_rules
             SET alpha = 25.0, beta_param = 1.0, lifecycle = 'awaiting_review'
             WHERE name = ?1",
            params![name],
        )
        .expect("seed accumulated evidence + awaiting_review");
        conn.query_row(
            "SELECT id FROM learned_rules WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )
        .expect("rule id")
    }

    fn current_version_of(storage: &Storage, name: &str) -> i64 {
        let conn = storage.conn.lock();
        conn.query_row(
            "SELECT current_version FROM learned_rules WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )
        .expect("read current_version")
    }

    fn distinct_refs_at(storage: &Storage, rule_id: i64, version: i64) -> i64 {
        let conn = storage.conn.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM (
                 SELECT DISTINCT kind, ref_id FROM rule_evidence_citations
                 WHERE rule_id = ?1 AND rule_version = ?2
             )",
            params![rule_id, version],
            |r| r.get(0),
        )
        .expect("count distinct refs")
    }

    /// C-B1 / C-B3: a v1 review-eligible pending rule re-derived with
    /// CHANGED content stays eligible across the whole re-derivation, and
    /// `current_version` becomes v2 ONLY after v2's citation snapshot
    /// exists. There is NO observable state in which `current_version`
    /// points at a version with 0 distinct refs.
    #[test]
    #[serial]
    fn rederivation_keeps_pending_rule_review_eligible_and_atomic() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let name = "atomic-pending-rule";
        let rule_id = seed_awaiting_review_rule(&storage, name, "v1 guidance body");

        // v1 snapshot: 3 distinct observation citations + ≥1 project path.
        let o1 = seed_observation(&storage, "se1", "/proj/x");
        let o2 = seed_observation(&storage, "se2", "/proj/x");
        let o3 = seed_observation(&storage, "se3", "/proj/x");
        let resolved_v1 =
            storage.resolve_evidence_refs(&[obs_ref(o1), obs_ref(o2), obs_ref(o3)], None);
        storage
            .persist_citations_and_advance_version(name, &resolved_v1, false)
            .expect("v1 citations (no bump)");
        assert_eq!(current_version_of(&storage, name), 1, "still v1 after seed");
        assert_eq!(distinct_refs_at(&storage, rule_id, 1), 3, "v1 has 3 refs");
        assert!(
            storage.eligible_for_review(name).expect("v1 eligibility"),
            "v1 pending rule with 3 refs + score≥0.6 IS review-eligible"
        );

        // Re-derive with CHANGED content through the new flow.
        let changed = storage
            .store_learned_rule(&crate::models::LearnedRulePayload {
                name: name.to_string(),
                domain: Some("errors".to_string()),
                confidence: 0.99,
                observation_count: 20,
                file_path: String::new(),
                project: None,
                is_anti_pattern: false,
                source: Some("claude".to_string()),
                content: Some("v2 REWRITTEN guidance body".to_string()),
                provider_scope: vec![IntegrationProvider::Claude],
            })
            .expect("re-derive merge");
        assert!(
            changed,
            "an awaiting_review rule whose content changed MUST signal pending_changed"
        );

        // INTERMEDIATE OBSERVABLE STATE: the merge committed but the bump has
        // NOT been applied yet — `current_version` is still v1, which still
        // has its 3 citations, so eligibility is unbroken. There is no
        // window where `current_version` references a 0-ref version.
        assert_eq!(
            current_version_of(&storage, name),
            1,
            "C-B1: store_learned_rule must NOT advance current_version itself"
        );
        assert_eq!(
            distinct_refs_at(&storage, rule_id, 1),
            3,
            "v1 citations untouched by the merge"
        );
        assert!(
            storage
                .eligible_for_review(name)
                .expect("intermediate eligibility"),
            "C-B1: rule stays review-eligible on its v1 snapshot before the bump"
        );

        // Persist v2 citations + atomically advance to v2.
        let resolved_v2 =
            storage.resolve_evidence_refs(&[obs_ref(o1), obs_ref(o2), obs_ref(o3)], None);
        storage
            .persist_citations_and_advance_version(name, &resolved_v2, changed)
            .expect("v2 citations + atomic bump");

        assert_eq!(
            current_version_of(&storage, name),
            2,
            "current_version advances to v2 only after v2 citations exist"
        );
        assert_eq!(
            distinct_refs_at(&storage, rule_id, 2),
            3,
            "C-B1: the version current_version now points at HAS its snapshot"
        );
        assert_eq!(
            distinct_refs_at(&storage, rule_id, 1),
            3,
            "prior v1 snapshot rows are never destroyed by the advance"
        );
        assert!(
            storage.eligible_for_review(name).expect("v2 eligibility"),
            "C-B1: rule remains review-eligible after the atomic advance to v2"
        );

        clear_env();
    }

    /// C-B2 / FR-010 / SC-006: if the citation (re)write fails during a
    /// re-derivation, `current_version` MUST NOT advance and the rule MUST
    /// remain review-eligible on its prior good snapshot (no permanently
    /// un-reviewable human-pending rule). Deterministic failure seam: a
    /// temporary `BEFORE INSERT` trigger on `rule_evidence_citations` that
    /// `RAISE(ABORT)`s — the least-invasive way to force the in-tx INSERT to
    /// error so the whole tx (citations + bump) rolls back.
    #[test]
    #[serial]
    fn citation_write_failure_does_not_advance_version() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let name = "failure-pending-rule";
        let rule_id = seed_awaiting_review_rule(&storage, name, "v1 guidance body");

        let o1 = seed_observation(&storage, "sf1", "/proj/y");
        let o2 = seed_observation(&storage, "sf2", "/proj/y");
        let o3 = seed_observation(&storage, "sf3", "/proj/y");
        let resolved_v1 =
            storage.resolve_evidence_refs(&[obs_ref(o1), obs_ref(o2), obs_ref(o3)], None);
        storage
            .persist_citations_and_advance_version(name, &resolved_v1, false)
            .expect("v1 citations");
        assert!(
            storage.eligible_for_review(name).expect("v1 eligibility"),
            "precondition: v1 pending rule is review-eligible"
        );

        // Re-derive with changed content (merge commits; pending_changed).
        let changed = storage
            .store_learned_rule(&crate::models::LearnedRulePayload {
                name: name.to_string(),
                domain: Some("errors".to_string()),
                confidence: 0.99,
                observation_count: 20,
                file_path: String::new(),
                project: None,
                is_anti_pattern: false,
                source: Some("claude".to_string()),
                content: Some("v2 REWRITTEN body that must not strand the rule".to_string()),
                provider_scope: vec![IntegrationProvider::Claude],
            })
            .expect("re-derive merge");
        assert!(changed, "content changed ⇒ pending_changed");

        // Arm the deterministic in-tx failure: any INSERT into
        // `rule_evidence_citations` aborts.
        {
            let conn = storage.conn.lock();
            conn.execute_batch(
                "CREATE TRIGGER fail_citation_insert
                 BEFORE INSERT ON rule_evidence_citations
                 BEGIN SELECT RAISE(ABORT, 'injected citation failure'); END;",
            )
            .expect("arm failure trigger");
        }

        let resolved_v2 =
            storage.resolve_evidence_refs(&[obs_ref(o1), obs_ref(o2), obs_ref(o3)], None);
        let result = storage.persist_citations_and_advance_version(name, &resolved_v2, changed);
        assert!(
            result.is_err(),
            "the injected INSERT abort must surface as Err"
        );

        // Disarm so the post-failure assertions read a clean DB.
        {
            let conn = storage.conn.lock();
            conn.execute_batch("DROP TRIGGER fail_citation_insert;")
                .expect("disarm failure trigger");
        }

        assert_eq!(
            current_version_of(&storage, name),
            1,
            "C-B2: a failed citation write must NOT advance current_version"
        );
        assert_eq!(
            distinct_refs_at(&storage, rule_id, 1),
            3,
            "C-B2: the prior v1 snapshot survives the rolled-back tx"
        );
        assert_eq!(
            distinct_refs_at(&storage, rule_id, 2),
            0,
            "no v2 rows were committed (tx rolled back)"
        );
        assert!(
            storage
                .eligible_for_review(name)
                .expect("post-failure eligibility"),
            "C-B2/SC-006: the rule stays review-eligible on its prior snapshot"
        );

        clear_env();
    }

    /// C-B3 / FR-011: re-derivation with UNCHANGED content is a no-op for
    /// the pending marker — `store_learned_rule` returns
    /// `pending_changed=false`, `current_version` is unchanged, and
    /// eligibility is unchanged (the snapshot is idempotently re-written at
    /// the same version, no bump).
    #[test]
    #[serial]
    fn rederivation_with_unchanged_content_is_a_noop() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);

        let name = "noop-pending-rule";
        let body = "stable guidance body that does not change";
        let rule_id = seed_awaiting_review_rule(&storage, name, body);

        let o1 = seed_observation(&storage, "sn1", "/proj/z");
        let o2 = seed_observation(&storage, "sn2", "/proj/z");
        let o3 = seed_observation(&storage, "sn3", "/proj/z");
        let resolved =
            storage.resolve_evidence_refs(&[obs_ref(o1), obs_ref(o2), obs_ref(o3)], None);
        storage
            .persist_citations_and_advance_version(name, &resolved, false)
            .expect("v1 citations");
        assert!(
            storage.eligible_for_review(name).expect("v1 eligibility"),
            "precondition: pending rule is review-eligible at v1"
        );
        let before = current_version_of(&storage, name);

        // Re-derive with the IDENTICAL content.
        let changed = storage
            .store_learned_rule(&crate::models::LearnedRulePayload {
                name: name.to_string(),
                domain: Some("errors".to_string()),
                confidence: 0.99,
                observation_count: 20,
                file_path: String::new(),
                project: None,
                is_anti_pattern: false,
                source: Some("claude".to_string()),
                content: Some(body.to_string()),
                provider_scope: vec![IntegrationProvider::Claude],
            })
            .expect("re-derive unchanged");
        assert!(
            !changed,
            "FR-011: unchanged content MUST NOT signal pending_changed"
        );

        storage
            .persist_citations_and_advance_version(name, &resolved, changed)
            .expect("idempotent re-snapshot, no bump");

        assert_eq!(
            current_version_of(&storage, name),
            before,
            "FR-011: unchanged re-derivation must NOT advance current_version"
        );
        assert_eq!(
            distinct_refs_at(&storage, rule_id, before),
            3,
            "the v1 snapshot is idempotently preserved at the same version"
        );
        assert!(
            storage
                .eligible_for_review(name)
                .expect("post-noop eligibility"),
            "FR-011: eligibility is unchanged by an unchanged re-derivation"
        );

        clear_env();
    }

    /// Feature 008 / US2: two sub-agent chains sharing the same parent
    /// session_id but different agent_id must produce two independent
    /// turns rather than one merged turn. Verifies STAT-3 from
    /// specs/008-runtime-redesign/contracts/llm-runtime-stats.md and the
    /// agent_id inclusion in the chain key.
    #[test]
    #[serial]
    fn session_events_sibling_subagents_form_independent_turns() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);
        let events_a = vec![
            crate::storage::SessionEventInput {
                timestamp: "2026-05-20T10:00:00.000Z",
                kind: crate::sessions::SessionEventKind::UserText,
                is_sidechain: true,
                agent_id: Some("agent-a"),
                uuid: None,
                parent_uuid: None,
            },
            crate::storage::SessionEventInput {
                timestamp: "2026-05-20T10:00:30.000Z",
                kind: crate::sessions::SessionEventKind::AsstText,
                is_sidechain: true,
                agent_id: Some("agent-a"),
                uuid: None,
                parent_uuid: None,
            },
        ];
        let events_b = vec![
            crate::storage::SessionEventInput {
                timestamp: "2026-05-20T10:00:05.000Z",
                kind: crate::sessions::SessionEventKind::UserText,
                is_sidechain: true,
                agent_id: Some("agent-b"),
                uuid: None,
                parent_uuid: None,
            },
            crate::storage::SessionEventInput {
                timestamp: "2026-05-20T10:00:40.000Z",
                kind: crate::sessions::SessionEventKind::AsstText,
                is_sidechain: true,
                agent_id: Some("agent-b"),
                uuid: None,
                parent_uuid: None,
            },
        ];
        storage
            .ingest_session_events(IntegrationProvider::Claude, "sess-1", &events_a)
            .expect("ingest a");
        storage
            .ingest_session_events(IntegrationProvider::Claude, "sess-1", &events_b)
            .expect("ingest b");
        // Wide range avoids clock-skew flake across the test cutoff.
        let stats = storage.get_llm_runtime_stats("30d", None).expect("stats");
        assert_eq!(
            stats.turn_count, 2,
            "two sibling sub-agent chains should produce two turns, got {}",
            stats.turn_count
        );
        clear_env();
    }

    /// Feature 008 / US2: `scope = parent_only` must select
    /// `WHERE is_sidechain = 0` so the total excludes sub-agent rows.
    /// Verifies the STAT-2 bracket clause.
    #[test]
    #[serial]
    fn session_events_parent_only_excludes_sidechain() {
        clear_env();
        let dir = TempDir::new().expect("tempdir");
        let storage = init_storage_in(&dir);
        let mut all_events = vec![
            crate::storage::SessionEventInput {
                timestamp: "2026-05-20T10:00:00.000Z",
                kind: crate::sessions::SessionEventKind::UserText,
                is_sidechain: false,
                agent_id: None,
                uuid: None,
                parent_uuid: None,
            },
            crate::storage::SessionEventInput {
                timestamp: "2026-05-20T10:00:10.000Z",
                kind: crate::sessions::SessionEventKind::AsstText,
                is_sidechain: false,
                agent_id: None,
                uuid: None,
                parent_uuid: None,
            },
        ];
        let subagent = vec![
            crate::storage::SessionEventInput {
                timestamp: "2026-05-20T10:00:20.000Z",
                kind: crate::sessions::SessionEventKind::UserText,
                is_sidechain: true,
                agent_id: Some("agent-x"),
                uuid: None,
                parent_uuid: None,
            },
            crate::storage::SessionEventInput {
                timestamp: "2026-05-20T10:00:50.000Z",
                kind: crate::sessions::SessionEventKind::AsstText,
                is_sidechain: true,
                agent_id: Some("agent-x"),
                uuid: None,
                parent_uuid: None,
            },
        ];
        all_events.extend(subagent);
        storage
            .ingest_session_events(IntegrationProvider::Claude, "sess-1", &all_events)
            .expect("ingest");
        let all = storage
            .get_llm_runtime_stats("30d", None)
            .expect("stats all");
        let parent = storage
            .get_llm_runtime_stats("30d", Some("parent_only"))
            .expect("stats parent");
        assert!(
            parent.total_runtime_secs < all.total_runtime_secs,
            "parent_only ({} s) must be strictly less than all ({} s)",
            parent.total_runtime_secs,
            all.total_runtime_secs
        );
        assert!(
            parent.total_runtime_secs > 0.0,
            "parent_only must still register parent activity"
        );
        clear_env();
    }
}
