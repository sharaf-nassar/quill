//! Read-only benchmark protocol for feature 020 widget queries.
//!
//! The spike binary opens a fresh [`Storage`] reader for every measurement,
//! bypassing all app-level caches while preserving the production query and
//! post-processing paths. A thread-local clock pins every range to one exact
//! endpoint. The frozen corpus is never migrated, cleaned up, or written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::integrations::IntegrationProvider;
use crate::models::{LlmRuntimeStats, ModelRange, UsageBucket};
use crate::rollup_backfill::{
    RollupBackfillControls, RollupBackfillProgress, RollupBackfillTerminal, RollupChunkControl,
};
use crate::storage::{
    DatabaseAnalysisResult, Storage, WidgetQueryTraceStatement, begin_widget_query_trace,
    finish_widget_query_trace, set_widget_query_trace_path, with_model_overview_stage_timings,
    with_pinned_query_now,
};

/// One cold, app-cache-bypassed endpoint measurement.
#[derive(Clone, Debug, Serialize)]
pub struct WidgetQueryMeasurement {
    pub query: &'static str,
    pub window: &'static str,
    pub start: String,
    pub end: String,
    pub elapsed_ms: f64,
    pub warm_elapsed_ms: f64,
    pub output_bytes: usize,
    pub output_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct WidgetQueryOutputHash {
    pub query: &'static str,
    pub window: &'static str,
    pub output_bytes: usize,
    pub output_sha256: String,
}

/// One opt-in internal stage from a cold model-overview call.
#[derive(Debug, Serialize)]
pub struct ModelOverviewStageMeasurement {
    pub stage: &'static str,
    pub elapsed_ms: f64,
}

/// Cold endpoint total plus internal production-stage attribution.
#[derive(Debug, Serialize)]
pub struct ModelOverviewProfile {
    pub window: &'static str,
    pub start: String,
    pub end: String,
    pub elapsed_ms: f64,
    pub output_bytes: usize,
    pub output_sha256: String,
    pub stages: Vec<ModelOverviewStageMeasurement>,
}

/// One complete view's backend request set, measured without frontend work.
#[derive(Clone, Debug, Serialize)]
pub struct WidgetViewFanoutMeasurement {
    pub view: &'static str,
    pub window: &'static str,
    pub calls: &'static str,
    pub cold_elapsed_ms: f64,
    pub warm_elapsed_ms: f64,
    pub output_bytes: usize,
}

/// Reproducible metadata and all baseline measurements from one corpus pass.
#[derive(Clone, Debug, Serialize)]
pub struct WidgetQueryBenchmarkReport {
    pub corpus_path: String,
    pub corpus_bytes: u64,
    pub corpus_read_only: bool,
    pub sqlite_version: String,
    pub schema_version: i64,
    pub page_size: i64,
    pub page_count: i64,
    pub freelist_count: i64,
    pub pinned_end: String,
    pub os_page_cache: &'static str,
    pub measurements: Vec<WidgetQueryMeasurement>,
    pub view_fanouts: Vec<WidgetViewFanoutMeasurement>,
}

#[derive(Debug, Serialize)]
pub struct QueryPlannerStatsSnapshot {
    pub exists: bool,
    pub rows: i64,
    pub sha256: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WidgetQueryPlanAuditEntry {
    pub path: String,
    pub connection_id: u64,
    pub sequence: usize,
    pub sql_shape_sha256: String,
    pub sql_shape: String,
    pub before_expanded_sha256: String,
    pub after_expanded_sha256: String,
    pub before_plan: Vec<String>,
    pub after_plan: Vec<String>,
    pub plan_changed: bool,
    pub regression_reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WidgetQueryTimingComparison {
    pub path: String,
    pub before_cold_ms: f64,
    pub after_cold_ms: f64,
    pub before_warm_ms: f64,
    pub after_warm_ms: f64,
    pub cold_delta_ms: f64,
    pub cold_ratio: f64,
    pub material_regression: bool,
}

#[derive(Debug, Serialize)]
pub struct WidgetQueryPlanAuditReport {
    pub corpus_path: String,
    pub corpus_bytes_before: u64,
    pub corpus_bytes_after: u64,
    pub pinned_end: String,
    pub sqlite_version: String,
    pub before_stats: QueryPlannerStatsSnapshot,
    pub analysis: DatabaseAnalysisResult,
    pub after_stats: QueryPlannerStatsSnapshot,
    pub audited_sql_statements: usize,
    pub changed_plans: usize,
    pub plan_regressions: usize,
    pub timing_regressions: usize,
    pub plans: Vec<WidgetQueryPlanAuditEntry>,
    pub timings: Vec<WidgetQueryTimingComparison>,
    pub before_benchmark: WidgetQueryBenchmarkReport,
    pub after_benchmark: WidgetQueryBenchmarkReport,
    pub quick_check: String,
    pub verdict: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ModelHistoryAbSample {
    pub state: &'static str,
    pub ordinal: usize,
    pub cold_ms: f64,
    pub warm_ms: f64,
    pub output_bytes: usize,
    pub output_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct ModelHistoryAbReport {
    pub corpus_path: String,
    pub pinned_end: String,
    pub samples_per_state: usize,
    pub initial_stats: QueryPlannerStatsSnapshot,
    pub statless_samples: Vec<ModelHistoryAbSample>,
    pub analyzed_samples: Vec<ModelHistoryAbSample>,
    pub statless_median_cold_ms: f64,
    pub analyzed_median_cold_ms: f64,
    pub median_delta_ms: f64,
    pub median_ratio: f64,
    pub material_regression: bool,
    pub plan_comparison: Vec<WidgetQueryPlanAuditEntry>,
    pub final_analysis: DatabaseAnalysisResult,
    pub final_stats: QueryPlannerStatsSnapshot,
    pub quick_check: String,
    pub verdict: &'static str,
}

#[derive(Debug, Serialize)]
pub struct FlaggedPathAbSample {
    pub state: &'static str,
    pub ordinal: usize,
    pub cold_ms: f64,
    pub warm_ms: f64,
    pub output_bytes: usize,
    pub output_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct FlaggedPathAbComparison {
    pub path: String,
    pub statless_samples: Vec<FlaggedPathAbSample>,
    pub analyzed_samples: Vec<FlaggedPathAbSample>,
    pub statless_median_cold_ms: f64,
    pub analyzed_median_cold_ms: f64,
    pub median_delta_ms: f64,
    pub median_ratio: f64,
    pub material_regression: bool,
    pub plan_comparison: Vec<WidgetQueryPlanAuditEntry>,
}

#[derive(Debug, Serialize)]
pub struct FlaggedPathsAbReport {
    pub corpus_path: String,
    pub pinned_end: String,
    pub samples_per_state: usize,
    pub initial_stats: QueryPlannerStatsSnapshot,
    pub comparisons: Vec<FlaggedPathAbComparison>,
    pub final_analysis: DatabaseAnalysisResult,
    pub final_stats: QueryPlannerStatsSnapshot,
    pub quick_check: String,
    pub verdict: &'static str,
}

/// Focused repeated measurement for the 30-day session-breakdown budget.
#[derive(Debug, Serialize)]
pub struct SessionBreakdownBenchmarkReport {
    pub corpus_path: String,
    pub corpus_bytes: u64,
    pub corpus_read_only: bool,
    pub pinned_end: String,
    pub range_start: String,
    pub samples_ms: Vec<f64>,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub output_bytes: usize,
    pub query_plan: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelRollupBackfillCorpusReport {
    pub rows_done: u64,
    pub rows_total: u64,
    pub chunks: u64,
    pub first_terminal: String,
    pub first_rows_done: u64,
    pub resume_bookmark_ms: Option<i64>,
    pub final_terminal: String,
    pub elapsed_ms: f64,
    pub max_chunk_interval_ms: f64,
    pub max_wal_bytes_after_checkpoint: u64,
    pub wal_exists_at_finish: bool,
    pub wal_bytes_at_finish: u64,
    pub shm_exists_at_finish: bool,
    pub raw_pruned_before: u64,
    pub raw_pruned_after: u64,
    pub missing_or_mismatched_rollup_rows: u64,
    pub extra_rollup_rows: u64,
    pub committed_status: String,
}

#[derive(Debug, Serialize)]
pub struct ModelRollupCorpusEquality {
    pub window: &'static str,
    pub overview_bytes: usize,
    pub overview_sha256: String,
    pub history_bytes: usize,
    pub history_sha256: String,
    pub exact: bool,
}

#[derive(Debug, Serialize)]
pub struct ModelRollupConsistencyCorpusReport {
    pub pinned_end: String,
    pub quick_check: String,
    pub backfill: ModelRollupBackfillCorpusReport,
    pub equality: Vec<ModelRollupCorpusEquality>,
}

#[derive(Debug, Serialize)]
pub struct ModelRollupDerivedCorpusReport {
    pub source_path: String,
    pub source_bytes: u64,
    pub source_read_only: bool,
    pub range_start: String,
    pub range_end: String,
    pub copied_sources: u64,
    pub copied_observations: u64,
    pub available_bytes_before: u64,
    pub required_bytes: u64,
    pub fixture_bytes_before_backfill: u64,
    pub consistency: ModelRollupConsistencyCorpusReport,
}

#[derive(Debug, Serialize)]
pub struct RuntimeParityWindowReport {
    pub window: &'static str,
    pub scope: &'static str,
    pub total_runtime_secs: f64,
    pub turn_count: i64,
    pub session_count: i64,
    pub avg_per_turn_secs: f64,
    pub sparkline: Vec<f64>,
    pub normalized_bytes: usize,
    pub normalized_sha256: String,
    pub exact: bool,
    pub repeated_stable: bool,
}

#[derive(Debug, Serialize)]
pub struct RuntimeParityDerivedCorpusReport {
    pub source_path: String,
    pub source_bytes: u64,
    pub source_read_only: bool,
    pub range_start: String,
    pub copied_start: String,
    pub range_end: String,
    pub copied_sources: u64,
    pub copied_events: u64,
    pub available_bytes_before: u64,
    pub required_bytes: u64,
    pub fixture_bytes_before_backfill: u64,
    pub backfill_rows_done: u64,
    pub backfill_rows_total: u64,
    pub backfill_chunks: u64,
    pub backfill_elapsed_ms: f64,
    pub runtime_rollup_rows: u64,
    pub runtime_state_rows: u64,
    pub quick_check: String,
    pub equality: Vec<RuntimeParityWindowReport>,
}

#[derive(Clone, Debug)]
struct RuntimeReferenceEvent {
    timestamp_ms: i64,
    kind: String,
}

#[derive(Clone, Debug)]
struct RuntimeReferenceSource {
    provider: String,
    source_key: String,
    closed_session_id: String,
    open_session_id: String,
    chain_id: String,
    is_sidechain: bool,
    events: Vec<RuntimeReferenceEvent>,
}

#[derive(Clone, Debug)]
struct RuntimeReferenceTurn {
    start_ms: i64,
    end_ms: i64,
}

#[derive(Clone, Debug)]
struct RuntimeReferenceOpenTurn {
    start_ms: i64,
    last_event_ms: i64,
    last_kind: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct CanonicalRuntimeStats {
    total_runtime_micros: i64,
    turn_count: i64,
    session_count: i64,
    avg_per_turn_micros: i64,
    sparkline_micros: Vec<i64>,
}

const RUNTIME_REFERENCE_IDLE_MS: i64 = 5 * 60 * 1_000;
const RUNTIME_REFERENCE_TOOL_WAIT_MS: i64 = 6 * 60 * 60 * 1_000;
const RUNTIME_REFERENCE_HOUR_MS: i64 = 60 * 60 * 1_000;
const RUNTIME_PARITY_EPSILON_SECS: f64 = 0.000_001;

#[derive(Debug)]
struct ModelRollupCorpusOutput {
    window: &'static str,
    overview: serde_json::Value,
    history: serde_json::Value,
}

#[derive(Debug)]
struct ModelCorpusProgress {
    chunks: u64,
    last_tick: Instant,
    max_chunk_interval_ms: f64,
    max_wal_bytes: u64,
}

fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

#[derive(Clone, Copy)]
struct Window {
    label: &'static str,
    duration: TimeDelta,
    model_range: ModelRange,
    bucket_days: i32,
}

const WINDOWS: [Window; 3] = [
    Window {
        label: "24h",
        duration: TimeDelta::hours(24),
        model_range: ModelRange::TwentyFourHours,
        bucket_days: 1,
    },
    Window {
        label: "30d",
        duration: TimeDelta::days(30),
        model_range: ModelRange::ThirtyDays,
        bucket_days: 30,
    },
    Window {
        label: "90d",
        duration: TimeDelta::days(90),
        model_range: ModelRange::NinetyDays,
        bucket_days: 90,
    },
];

fn measure<T: Serialize>(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    window: Window,
    query: &'static str,
    operation: impl Fn(&Storage) -> Result<T, String>,
) -> Result<WidgetQueryMeasurement, String> {
    let storage = Storage::init_widget_query_benchmark(corpus)?;
    let (elapsed_ms, warm_elapsed_ms, output_bytes, output_sha256) =
        with_pinned_query_now(pinned_end, || {
            set_widget_query_trace_path(format!("query/{}/{query}/cold", window.label));
            let started = Instant::now();
            let value = operation(&storage)?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let output = serde_json::to_vec(&value)
                .map_err(|error| format!("Serialize {query} benchmark output: {error}"))?;

            set_widget_query_trace_path(format!("query/{}/{query}/warm", window.label));
            let warm_started = Instant::now();
            let warm_value = operation(&storage)?;
            let warm_elapsed_ms = warm_started.elapsed().as_secs_f64() * 1_000.0;
            let warm_output = serde_json::to_vec(&warm_value)
                .map_err(|error| format!("Serialize warm {query} benchmark output: {error}"))?;
            if output != warm_output {
                return Err(format!(
                    "Cold and warm {query} benchmark outputs differ at pinned endpoint"
                ));
            }
            let output_sha256 = hex::encode(Sha256::digest(&output));
            Ok::<_, String>((elapsed_ms, warm_elapsed_ms, output.len(), output_sha256))
        })?;

    Ok(WidgetQueryMeasurement {
        query,
        window: window.label,
        start: (pinned_end - window.duration).to_rfc3339(),
        end: pinned_end.to_rfc3339(),
        elapsed_ms,
        warm_elapsed_ms,
        output_bytes,
        output_sha256,
    })
}

/// Measure only `get_session_breakdown` repeatedly on fresh read-only handles.
///
/// This keeps the slice-E acceptance run independent from the much slower
/// full query matrix while preserving the same pinned-clock and cache-bypass
/// rules. Every sample must serialize identically.
pub fn measure_session_breakdown_30d(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    sample_count: usize,
) -> Result<SessionBreakdownBenchmarkReport, String> {
    let sample_count = sample_count.clamp(1, 100);
    let canonical = corpus
        .canonicalize()
        .map_err(|error| format!("Resolve session breakdown corpus: {error}"))?;
    let metadata = canonical
        .metadata()
        .map_err(|error| format!("Read session breakdown corpus metadata: {error}"))?;
    if !metadata.is_file() {
        return Err(format!(
            "Session breakdown corpus is not a file: {}",
            canonical.display()
        ));
    }
    if !metadata.permissions().readonly() {
        return Err(format!(
            "Session breakdown corpus must be read-only: {}",
            canonical.display()
        ));
    }

    let plan_storage = Storage::init_widget_query_benchmark(&canonical)?;
    let query_plan = with_pinned_query_now(pinned_end, || {
        plan_storage.explain_session_breakdown_query("30d", None, None, Some(200))
    })?;
    drop(plan_storage);

    let mut samples_ms = Vec::with_capacity(sample_count);
    let mut canonical_output: Option<Vec<u8>> = None;
    for _ in 0..sample_count {
        let storage = Storage::init_widget_query_benchmark(&canonical)?;
        let (elapsed_ms, output) = with_pinned_query_now(pinned_end, || {
            let started = Instant::now();
            let value = storage.get_session_breakdown("30d", None, None, Some(200))?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let output = serde_json::to_vec(&value)
                .map_err(|error| format!("Serialize focused session breakdown output: {error}"))?;
            Ok::<_, String>((elapsed_ms, output))
        })?;
        if let Some(expected) = &canonical_output {
            if expected != &output {
                return Err(
                    "Focused session breakdown outputs differ at the pinned endpoint".to_string(),
                );
            }
        } else {
            canonical_output = Some(output);
        }
        samples_ms.push(elapsed_ms);
    }

    let mut ordered = samples_ms.clone();
    ordered.sort_by(f64::total_cmp);
    let p95_index = ((ordered.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(ordered.len() - 1);
    let p95_ms = ordered[p95_index];
    let max_ms = *ordered.last().expect("sample count is clamped above zero");
    let output_bytes = canonical_output.map_or(0, |output| output.len());

    Ok(SessionBreakdownBenchmarkReport {
        corpus_path: canonical.display().to_string(),
        corpus_bytes: metadata.len(),
        corpus_read_only: metadata.permissions().readonly(),
        pinned_end: pinned_end.to_rfc3339(),
        range_start: (pinned_end - TimeDelta::days(30)).to_rfc3339(),
        samples_ms,
        p95_ms,
        max_ms,
        output_bytes,
        query_plan,
    })
}

/// Measure completed-rollup model reads at their acceptance windows.
pub fn measure_model_rollup_queries(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<Vec<WidgetQueryMeasurement>, String> {
    let mut measurements = Vec::with_capacity(4);
    for window in WINDOWS.into_iter().skip(1) {
        measurements.push(measure(
            corpus,
            pinned_end,
            window,
            "get_model_usage_overview",
            |storage| storage.get_model_usage_overview(window.model_range, None),
        )?);
        measurements.push(measure(
            corpus,
            pinned_end,
            window,
            "get_model_history",
            |storage| storage.get_model_history(window.model_range, None, None),
        )?);
    }
    Ok(measurements)
}

/// Attribute one cold model-overview call without changing production output.
pub fn profile_model_overview_queries(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<Vec<ModelOverviewProfile>, String> {
    let mut profiles = Vec::with_capacity(2);
    for window in WINDOWS.into_iter().skip(1) {
        let storage = Storage::init_widget_query_benchmark(corpus)?;
        let (elapsed_ms, output, stages) = with_pinned_query_now(pinned_end, || {
            let started = Instant::now();
            let (value, stages) = with_model_overview_stage_timings(|| {
                storage.get_model_usage_overview(window.model_range, None)
            });
            let value = value?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let output = serde_json::to_vec(&value)
                .map_err(|error| format!("Serialize profiled model overview output: {error}"))?;
            Ok::<_, String>((elapsed_ms, output, stages))
        })?;
        profiles.push(ModelOverviewProfile {
            window: window.label,
            start: (pinned_end - window.duration).to_rfc3339(),
            end: pinned_end.to_rfc3339(),
            elapsed_ms,
            output_bytes: output.len(),
            output_sha256: hex::encode(Sha256::digest(&output)),
            stages: stages
                .into_iter()
                .map(|stage| ModelOverviewStageMeasurement {
                    stage: stage.stage,
                    elapsed_ms: stage.elapsed_ms,
                })
                .collect(),
        });
    }
    Ok(profiles)
}

fn hash_output<T: Serialize>(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    window: Window,
    query: &'static str,
    operation: impl Fn(&Storage) -> Result<T, String>,
) -> Result<WidgetQueryOutputHash, String> {
    let storage = Storage::init_widget_query_benchmark(corpus)?;
    let output = with_pinned_query_now(pinned_end, || {
        let value = operation(&storage)?;
        serde_json::to_vec(&value)
            .map_err(|error| format!("Serialize {query} parity output: {error}"))
    })?;
    Ok(WidgetQueryOutputHash {
        query,
        window: window.label,
        output_bytes: output.len(),
        output_sha256: hex::encode(Sha256::digest(&output)),
    })
}

/// Hash completed-rollup model responses without app-cache warm samples.
pub fn hash_model_rollup_queries(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<Vec<WidgetQueryOutputHash>, String> {
    let mut hashes = Vec::with_capacity(4);
    for window in WINDOWS.into_iter().skip(1) {
        hashes.push(hash_output(
            corpus,
            pinned_end,
            window,
            "get_model_usage_overview",
            |storage| storage.get_model_usage_overview(window.model_range, None),
        )?);
        hashes.push(hash_output(
            corpus,
            pinned_end,
            window,
            "get_model_history",
            |storage| storage.get_model_history(window.model_range, None, None),
        )?);
    }
    Ok(hashes)
}

fn add_output<T: Serialize>(
    total: &mut usize,
    query: &str,
    result: Result<T, String>,
) -> Result<(), String> {
    let value = result?;
    *total += serde_json::to_vec(&value)
        .map_err(|error| format!("Serialize {query} fan-out output: {error}"))?
        .len();
    Ok(())
}

fn normalized_model_rollup_output<T: Serialize>(
    value: &T,
    label: &str,
) -> Result<serde_json::Value, String> {
    let mut value = serde_json::to_value(value)
        .map_err(|error| format!("Serialize {label} consistency output: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| format!("{label} consistency output was not an object"))?;
    object.remove("buildingIndex");
    // Fresh derived fixtures stamp their migration-28 singleton at creation.
    // That lifecycle timestamp is unrelated to query semantics and otherwise
    // makes cross-fixture overview digests differ despite exact raw/hybrid data.
    if let Some(backfill) = object
        .get_mut("backfill")
        .and_then(serde_json::Value::as_object_mut)
    {
        backfill.remove("updatedAt");
    }
    Ok(value)
}

fn collect_model_rollup_outputs(
    storage: &Storage,
    pinned_end: DateTime<Utc>,
) -> Result<Vec<ModelRollupCorpusOutput>, String> {
    with_pinned_query_now(pinned_end, || {
        WINDOWS
            .iter()
            .map(|window| {
                let overview = storage.get_model_usage_overview(window.model_range, None)?;
                let history = storage.get_model_history(window.model_range, None, None)?;
                Ok(ModelRollupCorpusOutput {
                    window: window.label,
                    overview: normalized_model_rollup_output(
                        &overview,
                        &format!("{} overview", window.label),
                    )?,
                    history: normalized_model_rollup_output(
                        &history,
                        &format!("{} history", window.label),
                    )?,
                })
            })
            .collect()
    })
}

fn exact_output_digest(
    raw: &serde_json::Value,
    hybrid: &serde_json::Value,
    window: &str,
    output: &str,
) -> Result<(usize, String), String> {
    if raw != hybrid {
        return Err(format!(
            "Frozen corpus {window} {output} differs between raw and hybrid reads"
        ));
    }
    let bytes = serde_json::to_vec(raw)
        .map_err(|error| format!("Serialize {window} {output} equality digest: {error}"))?;
    Ok((bytes.len(), format!("{:x}", Sha256::digest(&bytes))))
}

fn runtime_gap_continues(previous_kind: &str, next_kind: &str, gap_ms: i64) -> bool {
    if previous_kind == "asst_tool_use" && next_kind == "user_tool_result" {
        gap_ms <= RUNTIME_REFERENCE_TOOL_WAIT_MS
    } else {
        gap_ms <= RUNTIME_REFERENCE_IDLE_MS
    }
}

fn fold_runtime_reference_source(
    source: &RuntimeReferenceSource,
) -> (Vec<RuntimeReferenceTurn>, Option<RuntimeReferenceOpenTurn>) {
    let Some(first) = source.events.first() else {
        return (Vec::new(), None);
    };
    let mut turns = Vec::new();
    let mut turn_start_ms = first.timestamp_ms;
    let mut previous = first;
    for event in source.events.iter().skip(1) {
        let gap_ms = event.timestamp_ms.saturating_sub(previous.timestamp_ms);
        if !runtime_gap_continues(&previous.kind, &event.kind, gap_ms) {
            let end_ms = if previous.kind == "asst_tool_use" && event.kind == "user_tool_result" {
                previous
                    .timestamp_ms
                    .saturating_add(gap_ms.min(RUNTIME_REFERENCE_TOOL_WAIT_MS))
            } else {
                previous.timestamp_ms
            };
            if end_ms > turn_start_ms {
                turns.push(RuntimeReferenceTurn {
                    start_ms: turn_start_ms,
                    end_ms,
                });
            }
            turn_start_ms = event.timestamp_ms;
        }
        previous = event;
    }
    let open = Some(RuntimeReferenceOpenTurn {
        start_ms: turn_start_ms,
        last_event_ms: previous.timestamp_ms,
        last_kind: previous.kind.clone(),
    });
    (turns, open)
}

fn independent_runtime_reference(
    sources: &[RuntimeReferenceSource],
    duration: TimeDelta,
    pinned_now: DateTime<Utc>,
    parent_only: bool,
) -> LlmRuntimeStats {
    let from = pinned_now - duration;
    let from_ms = from.timestamp_millis();
    let window_start_hour =
        from_ms.div_euclid(RUNTIME_REFERENCE_HOUR_MS) * RUNTIME_REFERENCE_HOUR_MS;
    let bucket_secs = duration.num_seconds() as f64 / 7.0;
    let mut total_runtime_secs = 0.0_f64;
    let mut turn_count = 0_i64;
    let mut sessions = std::collections::HashSet::<(String, String)>::new();
    let mut sparkline = vec![0.0_f64; 7];

    for source in sources {
        if parent_only && source.is_sidechain {
            continue;
        }
        let (closed, open) = fold_runtime_reference_source(source);
        let mut closed_hours = std::collections::BTreeMap::<i64, (i64, i64)>::new();
        for turn in closed {
            let hour =
                turn.start_ms.div_euclid(RUNTIME_REFERENCE_HOUR_MS) * RUNTIME_REFERENCE_HOUR_MS;
            if hour < window_start_hour {
                continue;
            }
            let duration_ms = turn.end_ms.saturating_sub(turn.start_ms);
            let entry = closed_hours.entry(hour).or_insert((0, 0));
            entry.0 = entry.0.saturating_add(duration_ms);
            entry.1 = entry.1.saturating_add(1);
        }
        for (hour, (duration_ms, turns)) in closed_hours {
            let duration_secs = duration_ms as f64 / 1_000.0;
            total_runtime_secs += duration_secs;
            turn_count += turns;
            sessions.insert((source.provider.clone(), source.closed_session_id.clone()));
            let bucket = ((((hour - from_ms) as f64 / 1_000.0) / bucket_secs).max(0.0)) as usize;
            sparkline[bucket.min(6)] += duration_secs;
        }

        let Some(open) = open.filter(|turn| turn.start_ms >= window_start_hour) else {
            continue;
        };
        sessions.insert((source.provider.clone(), source.open_session_id.clone()));
        let end_ms = if open.last_kind == "asst_tool_use" {
            pinned_now.timestamp_millis().min(
                open.last_event_ms
                    .saturating_add(RUNTIME_REFERENCE_TOOL_WAIT_MS),
            )
        } else {
            open.last_event_ms
        };
        let duration_ms = end_ms.saturating_sub(open.start_ms);
        if duration_ms > 0 {
            let duration_secs = duration_ms as f64 / 1_000.0;
            total_runtime_secs += duration_secs;
            turn_count += 1;
            let bucket =
                (((open.start_ms - from_ms) as f64 / 1_000.0) / bucket_secs).max(0.0) as usize;
            sparkline[bucket.min(6)] += duration_secs;
        }
    }

    LlmRuntimeStats {
        total_runtime_secs,
        turn_count,
        session_count: sessions.len() as i64,
        avg_per_turn_secs: if turn_count > 0 {
            total_runtime_secs / turn_count as f64
        } else {
            0.0
        },
        sparkline,
    }
}

fn load_runtime_reference_sources(
    conn: &rusqlite::Connection,
) -> Result<Vec<RuntimeReferenceSource>, String> {
    let mut statement = conn
        .prepare(
            "SELECT event.rowid, event.provider, event.source_key,
                    event.session_id, event.chain_id, event.timestamp, event.kind,
                    source.analytics_session_id, source.chain_id, source.is_sidechain
             FROM session_events AS event
             JOIN transcript_analytics_sources AS source
               ON source.provider = event.provider
              AND source.source_key = event.source_key
             WHERE event.source_key IS NOT NULL
               AND source.processing_status != 'suppressed'
               AND source.suppressed_sha256 IS NULL
             ORDER BY event.provider, event.source_key, event.timestamp, event.rowid",
        )
        .map_err(|error| format!("Prepare independent runtime source read: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })
        .map_err(|error| format!("Query independent runtime sources: {error}"))?;

    let mut sources = Vec::<RuntimeReferenceSource>::new();
    for row in rows {
        let (
            rowid,
            provider,
            source_key,
            session_id,
            chain_id,
            timestamp,
            kind,
            analytics_session_id,
            registry_chain_id,
            is_sidechain,
        ) = row.map_err(|error| format!("Read independent runtime event: {error}"))?;
        let Some(analytics_session_id) = analytics_session_id else {
            return Err(format!(
                "Runtime reference source {provider}/{source_key} lacks analytics_session_id"
            ));
        };
        if registry_chain_id.as_deref() != Some(chain_id.as_str()) {
            return Err(format!(
                "Runtime reference source {provider}/{source_key} registry chain differs from event chain"
            ));
        }
        let Ok(timestamp) = DateTime::parse_from_rfc3339(&timestamp) else {
            continue;
        };
        let timestamp_ms = timestamp.timestamp_millis();
        if timestamp_ms < 0 {
            return Err(format!(
                "Runtime reference event {provider}/{source_key}/{rowid} predates Unix epoch"
            ));
        }
        if sources
            .last()
            .is_none_or(|source| source.provider != provider || source.source_key != source_key)
        {
            sources.push(RuntimeReferenceSource {
                provider: provider.clone(),
                source_key: source_key.clone(),
                closed_session_id: session_id.clone(),
                open_session_id: analytics_session_id.clone(),
                chain_id: chain_id.clone(),
                is_sidechain: is_sidechain != 0,
                events: Vec::new(),
            });
        }
        let source = sources.last_mut().expect("source was inserted above");
        if source.closed_session_id != session_id
            || source.open_session_id != analytics_session_id
            || source.chain_id != chain_id
            || source.is_sidechain != (is_sidechain != 0)
        {
            return Err(format!(
                "Runtime reference source {provider}/{source_key} has mixed identity"
            ));
        }
        source
            .events
            .push(RuntimeReferenceEvent { timestamp_ms, kind });
    }
    Ok(sources)
}

fn normalized_runtime_micros(value: f64, field: &str) -> Result<i64, String> {
    if !value.is_finite() {
        return Err(format!("Runtime parity {field} is not finite"));
    }
    let micros = value * 1_000_000.0;
    if micros < i64::MIN as f64 || micros > i64::MAX as f64 {
        return Err(format!("Runtime parity {field} exceeds i64 microseconds"));
    }
    Ok(micros.round() as i64)
}

fn canonical_runtime_stats(stats: &LlmRuntimeStats) -> Result<CanonicalRuntimeStats, String> {
    Ok(CanonicalRuntimeStats {
        total_runtime_micros: normalized_runtime_micros(
            stats.total_runtime_secs,
            "total_runtime_secs",
        )?,
        turn_count: stats.turn_count,
        session_count: stats.session_count,
        avg_per_turn_micros: normalized_runtime_micros(
            stats.avg_per_turn_secs,
            "avg_per_turn_secs",
        )?,
        sparkline_micros: stats
            .sparkline
            .iter()
            .enumerate()
            .map(|(index, value)| normalized_runtime_micros(*value, &format!("sparkline[{index}]")))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn require_runtime_parity(
    reference: &LlmRuntimeStats,
    production: &LlmRuntimeStats,
    window: &str,
    scope: &str,
) -> Result<(usize, String), String> {
    if reference.turn_count != production.turn_count
        || reference.session_count != production.session_count
        || reference.sparkline.len() != production.sparkline.len()
    {
        return Err(format!(
            "Runtime parity count/shape mismatch at {window}/{scope}: reference={reference:?}, production={production:?}"
        ));
    }
    let close = |left: f64, right: f64| (left - right).abs() <= RUNTIME_PARITY_EPSILON_SECS;
    if !close(reference.total_runtime_secs, production.total_runtime_secs)
        || !close(reference.avg_per_turn_secs, production.avg_per_turn_secs)
        || reference
            .sparkline
            .iter()
            .zip(&production.sparkline)
            .any(|(left, right)| !close(*left, *right))
    {
        return Err(format!(
            "Runtime parity numeric mismatch at {window}/{scope}: reference={reference:?}, production={production:?}"
        ));
    }
    let reference = canonical_runtime_stats(reference)?;
    let production = canonical_runtime_stats(production)?;
    if reference != production {
        return Err(format!(
            "Runtime parity normalized mismatch at {window}/{scope}: reference={reference:?}, production={production:?}"
        ));
    }
    let bytes = serde_json::to_vec(&reference)
        .map_err(|error| format!("Serialize runtime parity digest at {window}/{scope}: {error}"))?;
    Ok((bytes.len(), format!("{:x}", Sha256::digest(&bytes))))
}

fn usage_view_fanout(storage: &Storage) -> Result<usize, String> {
    let mut bytes = 0;
    add_output(
        &mut bytes,
        "get_provider_token_series",
        storage.get_provider_token_series("30d", Some(8)),
    )?;
    add_output(
        &mut bytes,
        "get_activity_series",
        storage.get_activity_series("30d", Some(8)),
    )?;
    add_output(
        &mut bytes,
        "get_token_stats",
        storage.get_token_stats("30d", None, None, None, None),
    )?;
    add_output(
        &mut bytes,
        "get_llm_runtime_stats",
        storage.get_llm_runtime_stats("30d", None),
    )?;
    add_output(&mut bytes, "get_code_stats", storage.get_code_stats("30d"))?;
    add_output(
        &mut bytes,
        "get_code_stats_history (code stats)",
        storage.get_code_stats_history("30d"),
    )?;
    add_output(
        &mut bytes,
        "get_context_savings_analytics",
        storage.get_context_savings_analytics("30d", Some(40)),
    )?;
    add_output(
        &mut bytes,
        "get_retention_policy",
        storage.get_retention_policy(),
    )?;
    add_output(
        &mut bytes,
        "get_session_breakdown",
        storage.get_session_breakdown("30d", None, None, Some(200)),
    )?;
    add_output(
        &mut bytes,
        "get_project_breakdown",
        storage.get_project_breakdown("30d"),
    )?;
    add_output(
        &mut bytes,
        "get_token_history",
        storage.get_token_history("30d", None, None, None, None),
    )?;
    add_output(
        &mut bytes,
        "get_code_stats_history (insights)",
        storage.get_code_stats_history("30d"),
    )?;
    Ok(bytes)
}

fn charts_view_fanout(storage: &Storage) -> Result<usize, String> {
    let mut bytes = 0;
    add_output(
        &mut bytes,
        "get_provider_token_series",
        storage.get_provider_token_series("30d", Some(8)),
    )?;
    add_output(&mut bytes, "get_code_stats", storage.get_code_stats("30d"))?;
    add_output(
        &mut bytes,
        "get_code_stats_history",
        storage.get_code_stats_history("30d"),
    )?;
    add_output(
        &mut bytes,
        "get_token_history",
        storage.get_token_history("30d", None, None, None, None),
    )?;
    add_output(
        &mut bytes,
        "get_retention_policy",
        storage.get_retention_policy(),
    )?;
    Ok(bytes)
}

fn context_view_fanout(storage: &Storage) -> Result<usize, String> {
    let mut bytes = 0;
    add_output(
        &mut bytes,
        "get_context_savings_analytics",
        storage.get_context_savings_analytics("30d", Some(40)),
    )?;
    Ok(bytes)
}

fn measure_view_fanout(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    view: &'static str,
    calls: &'static str,
    operation: impl Fn(&Storage) -> Result<usize, String>,
) -> Result<WidgetViewFanoutMeasurement, String> {
    let storage = Storage::init_widget_query_benchmark(corpus)?;
    let (cold_elapsed_ms, warm_elapsed_ms, output_bytes) =
        with_pinned_query_now(pinned_end, || {
            set_widget_query_trace_path(format!("fanout/{view}/cold"));
            let started = Instant::now();
            let cold_output_bytes = operation(&storage)?;
            let cold_elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;

            set_widget_query_trace_path(format!("fanout/{view}/warm"));
            let warm_started = Instant::now();
            let warm_output_bytes = operation(&storage)?;
            let warm_elapsed_ms = warm_started.elapsed().as_secs_f64() * 1_000.0;
            if cold_output_bytes != warm_output_bytes {
                return Err(format!(
                    "Cold and warm {view} fan-out output sizes differ at pinned endpoint"
                ));
            }
            Ok::<_, String>((cold_elapsed_ms, warm_elapsed_ms, cold_output_bytes))
        })?;

    Ok(WidgetViewFanoutMeasurement {
        view,
        window: "30d",
        calls,
        cold_elapsed_ms,
        warm_elapsed_ms,
        output_bytes,
    })
}

fn latest_usage_buckets(corpus: &Path) -> Result<Vec<UsageBucket>, String> {
    let storage = Storage::init_widget_query_benchmark(corpus)?;
    let mut buckets = Vec::new();
    for provider in [
        IntegrationProvider::Claude,
        IntegrationProvider::Codex,
        IntegrationProvider::MiniMax,
    ] {
        buckets.extend(storage.get_latest_usage_buckets(provider)?);
    }
    Ok(buckets)
}

/// Backfill one worker-owned mutable corpus copy through the production target.
pub fn backfill_runtime_rollup_copy(corpus: &Path) -> Result<(u64, u64), String> {
    let metadata = corpus
        .metadata()
        .map_err(|error| format!("Read runtime backfill copy metadata: {error}"))?;
    if metadata.permissions().readonly() {
        return Err(format!(
            "Runtime backfill copy must be writable: {}",
            corpus.display()
        ));
    }
    let storage = Storage::init_study_scratch(corpus)?;
    let report = storage.run_runtime_rollup_backfill()?;
    if report.terminal != RollupBackfillTerminal::Completed {
        return Err(format!(
            "Runtime corpus backfill did not complete: {:?}",
            report.terminal
        ));
    }
    Ok((report.progress.rows_done, report.progress.rows_total))
}

/// Interrupt after one committed model chunk, resume, and prove corpus parity.
pub fn backfill_model_rollup_copy(
    corpus: &Path,
) -> Result<ModelRollupBackfillCorpusReport, String> {
    let metadata = corpus
        .metadata()
        .map_err(|error| format!("Read model backfill copy metadata: {error}"))?;
    if metadata.permissions().readonly() {
        return Err(format!(
            "Model backfill copy must be writable: {}",
            corpus.display()
        ));
    }
    let storage = Storage::init_study_scratch(corpus)?;
    let wal_path = sqlite_sidecar(corpus, "-wal");
    let shm_path = sqlite_sidecar(corpus, "-shm");
    let inspection = rusqlite::Connection::open(corpus)
        .map_err(|error| format!("Open model corpus inspection: {error}"))?;
    let raw_pruned_before = inspection
        .query_row(
            "SELECT COUNT(*) FROM model_usage_hourly WHERE raw_pruned = 1",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| format!("Count pre-backfill pruned model rows: {error}"))?;
    drop(inspection);

    let started = Instant::now();
    let progress_state = Mutex::new(ModelCorpusProgress {
        chunks: 0,
        last_tick: started,
        max_chunk_interval_ms: 0.0,
        max_wal_bytes: 0,
    });
    let progress = |_value: &RollupBackfillProgress| {
        let now = Instant::now();
        let mut state = progress_state.lock().expect("lock model corpus progress");
        state.chunks = state.chunks.saturating_add(1);
        state.max_chunk_interval_ms = state
            .max_chunk_interval_ms
            .max(now.duration_since(state.last_tick).as_secs_f64() * 1_000.0);
        state.last_tick = now;
        state.max_wal_bytes = state.max_wal_bytes.max(
            wal_path
                .metadata()
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        );
    };
    let interrupt = |_value: &RollupBackfillProgress| RollupChunkControl::Interrupt;
    let first_controls = RollupBackfillControls {
        progress: Some(&progress),
        after_chunk: Some(&interrupt),
        ..RollupBackfillControls::default()
    };
    let first = storage.run_model_rollup_backfill_with_controls(&first_controls)?;
    if first.terminal != RollupBackfillTerminal::Interrupted {
        return Err(format!(
            "Model corpus first pass did not interrupt after one chunk: {:?}",
            first.terminal
        ));
    }
    let resume_bookmark_ms = first.progress.done_through;
    let resumed_controls = RollupBackfillControls {
        progress: Some(&progress),
        ..RollupBackfillControls::default()
    };
    let final_report = storage.run_model_rollup_backfill_with_controls(&resumed_controls)?;
    if final_report.terminal != RollupBackfillTerminal::Completed {
        return Err(format!(
            "Model corpus backfill did not complete: {:?}",
            final_report.terminal
        ));
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;

    let inspection = rusqlite::Connection::open(corpus)
        .map_err(|error| format!("Reopen model corpus inspection: {error}"))?;
    let (missing_or_mismatched_rollup_rows, extra_rollup_rows) = inspection
        .query_row(
            "WITH raw AS (
                 SELECT (observed_at_ms / 3600000) * 3600000 AS hour_utc,
                        provider, COALESCE(derived_model_id, '') AS derived_model_id,
                        source_key, analytics_session_id, COUNT(*) AS obs_count,
                        SUM(observation_kind = 'turn') AS turn_count,
                        SUM(observation_kind = 'token') AS token_count,
                        SUM(is_sidechain) AS sidechain_count,
                        SUM(COALESCE(input_tokens, 0)) AS input_tokens,
                        COUNT(input_tokens) AS input_tokens_present,
                        SUM(COALESCE(output_tokens, 0)) AS output_tokens,
                        COUNT(output_tokens) AS output_tokens_present,
                        SUM(COALESCE(cache_creation_tokens, 0)) AS cache_creation_tokens,
                        COUNT(cache_creation_tokens) AS cache_creation_tokens_present,
                        SUM(COALESCE(cache_read_tokens, 0)) AS cache_read_tokens,
                        COUNT(cache_read_tokens) AS cache_read_tokens_present,
                        MIN(observed_at_ms) AS first_observed_at_ms,
                        MAX(observed_at_ms) AS last_observed_at_ms
                 FROM model_usage_observations
                 GROUP BY 1, 2, 3, 4, 5
             ), folded AS (
                 SELECT hour_utc, provider, derived_model_id, source_key,
                        analytics_session_id, obs_count, turn_count, token_count,
                        sidechain_count, input_tokens, input_tokens_present,
                        output_tokens, output_tokens_present,
                        cache_creation_tokens, cache_creation_tokens_present,
                        cache_read_tokens, cache_read_tokens_present,
                        first_observed_at_ms, last_observed_at_ms
                 FROM model_usage_hourly WHERE raw_pruned = 0
             ), missing AS (
                 SELECT * FROM raw EXCEPT SELECT * FROM folded
             ), extra AS (
                 SELECT * FROM folded EXCEPT SELECT * FROM raw
             )
             SELECT (SELECT COUNT(*) FROM missing),
                    (SELECT COUNT(*) FROM extra)",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        )
        .map_err(|error| format!("Compare model corpus raw and rollup rows: {error}"))?;
    let raw_pruned_after = inspection
        .query_row(
            "SELECT COUNT(*) FROM model_usage_hourly WHERE raw_pruned = 1",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| format!("Count post-backfill pruned model rows: {error}"))?;
    let committed_status = inspection
        .query_row(
            "SELECT model_backfill_status FROM rollup_meta WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| format!("Read committed model corpus status: {error}"))?;
    drop(inspection);

    let progress_state = progress_state
        .into_inner()
        .map_err(|_| "Model corpus progress lock was poisoned".to_string())?;
    let wal_bytes_at_finish = wal_path
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    Ok(ModelRollupBackfillCorpusReport {
        rows_done: final_report.progress.rows_done,
        rows_total: final_report.progress.rows_total,
        chunks: progress_state.chunks,
        first_terminal: format!("{:?}", first.terminal),
        first_rows_done: first.progress.rows_done,
        resume_bookmark_ms,
        final_terminal: format!("{:?}", final_report.terminal),
        elapsed_ms,
        max_chunk_interval_ms: progress_state.max_chunk_interval_ms,
        max_wal_bytes_after_checkpoint: progress_state.max_wal_bytes,
        wal_exists_at_finish: wal_path.exists(),
        wal_bytes_at_finish,
        shm_exists_at_finish: shm_path.exists(),
        raw_pruned_before,
        raw_pruned_after,
        missing_or_mismatched_rollup_rows,
        extra_rollup_rows,
        committed_status,
    })
}

/// Rebuild one writable corpus copy, then compare production hybrid reads to
/// the pre-rebuild raw path at one pinned endpoint.
pub fn verify_model_rollup_copy(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<ModelRollupConsistencyCorpusReport, String> {
    let metadata = corpus
        .metadata()
        .map_err(|error| format!("Read model consistency copy metadata: {error}"))?;
    if metadata.permissions().readonly() {
        return Err(format!(
            "Model consistency copy must be writable: {}",
            corpus.display()
        ));
    }

    let storage = Storage::init_study_scratch(corpus)?;
    let inspection = rusqlite::Connection::open(corpus)
        .map_err(|error| format!("Open model consistency inspection: {error}"))?;
    let quick_check = inspection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Quick-check model consistency copy: {error}"))?;
    if quick_check != "ok" {
        return Err(format!(
            "Model consistency copy quick_check failed: {quick_check}"
        ));
    }
    let authoritative_rows = inspection
        .query_row(
            "SELECT COUNT(*) FROM model_usage_hourly WHERE raw_pruned = 1",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| format!("Count authoritative model corpus rows: {error}"))?;
    drop(inspection);
    if authoritative_rows != 0 {
        return Err(format!(
            "Frozen corpus raw equality requires zero authoritative rows; found {authoritative_rows}"
        ));
    }

    storage.reset_model_rollup_backfill()?;
    let raw = collect_model_rollup_outputs(&storage, pinned_end)?;
    drop(storage);

    let backfill = backfill_model_rollup_copy(corpus)?;
    if backfill.missing_or_mismatched_rollup_rows != 0 || backfill.extra_rollup_rows != 0 {
        return Err(format!(
            "Frozen corpus rollup rows differ from raw refold: missing_or_mismatched={}, extra={}",
            backfill.missing_or_mismatched_rollup_rows, backfill.extra_rollup_rows
        ));
    }

    let storage = Storage::init_study_scratch(corpus)?;
    let hybrid = collect_model_rollup_outputs(&storage, pinned_end)?;
    if raw.len() != hybrid.len() {
        return Err("Frozen corpus raw and hybrid window counts differ".to_string());
    }
    let mut equality = Vec::with_capacity(raw.len());
    for (raw, hybrid) in raw.iter().zip(&hybrid) {
        if raw.window != hybrid.window {
            return Err(format!(
                "Frozen corpus raw window {} paired with hybrid window {}",
                raw.window, hybrid.window
            ));
        }
        let (overview_bytes, overview_sha256) =
            exact_output_digest(&raw.overview, &hybrid.overview, raw.window, "overview")?;
        let (history_bytes, history_sha256) =
            exact_output_digest(&raw.history, &hybrid.history, raw.window, "history")?;
        equality.push(ModelRollupCorpusEquality {
            window: raw.window,
            overview_bytes,
            overview_sha256,
            history_bytes,
            history_sha256,
            exact: true,
        });
    }

    Ok(ModelRollupConsistencyCorpusReport {
        pinned_end: pinned_end.to_rfc3339(),
        quick_check,
        backfill,
        equality,
    })
}

fn attached_table_columns(
    conn: &rusqlite::Connection,
    schema: &str,
    table: &str,
) -> Result<Vec<String>, String> {
    let sql = format!("PRAGMA {schema}.table_info('{table}')");
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| format!("Prepare {schema}.{table} layout read: {error}"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("Query {schema}.{table} layout: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Read {schema}.{table} layout: {error}"))?;
    Ok(columns)
}

/// Derive one bounded writable fixture from an immutable corpus attachment,
/// then exercise the production backfill and hybrid readers on that fixture.
// @lat: [[model-rollup-tests#Model Rollup Backfill Test Specs#Frozen Corpus Raw Hybrid Equality]]
pub fn verify_model_rollup_from_frozen(
    source: &Path,
    fixture: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<ModelRollupDerivedCorpusReport, String> {
    let source = source
        .canonicalize()
        .map_err(|error| format!("Resolve frozen model corpus: {error}"))?;
    let source_metadata = source
        .metadata()
        .map_err(|error| format!("Read frozen model corpus metadata: {error}"))?;
    if !source_metadata.is_file() || !source_metadata.permissions().readonly() {
        return Err(format!(
            "Frozen model corpus must be a read-only file: {}",
            source.display()
        ));
    }
    if fixture.exists() {
        return Err(format!(
            "Refusing to overwrite derived model fixture: {}",
            fixture.display()
        ));
    }
    let fixture_parent = fixture
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "Derived model fixture must have a parent directory".to_string())?;
    let available_bytes = crate::retention_engine::available_disk_space(fixture_parent)?;
    let required_bytes = source_metadata.len();
    if available_bytes < required_bytes {
        return Err(format!(
            "Derived model fixture needs at least {required_bytes} free bytes; found {available_bytes}"
        ));
    }

    let range_start = pinned_end - TimeDelta::days(90);
    let range_start_ms = range_start.timestamp_millis();
    let range_end_ms = pinned_end.timestamp_millis();
    let storage = Storage::init_study_scratch(fixture)?;
    drop(storage);

    let mut conn = rusqlite::Connection::open(fixture)
        .map_err(|error| format!("Open derived model fixture: {error}"))?;
    let source_path = source
        .to_str()
        .ok_or_else(|| "Frozen model corpus path is not UTF-8".to_string())?;
    if source_path.contains(['?', '#']) {
        return Err("Frozen model corpus path cannot contain URI query delimiters".to_string());
    }
    let source_uri = format!("file:{source_path}?mode=ro&immutable=1");
    conn.execute("ATTACH DATABASE ?1 AS frozen", [&source_uri])
        .map_err(|error| format!("Attach immutable model corpus: {error}"))?;
    for table in ["model_observation_sources", "model_usage_observations"] {
        let main_columns = attached_table_columns(&conn, "main", table)?;
        let frozen_columns = attached_table_columns(&conn, "frozen", table)?;
        if main_columns != frozen_columns {
            return Err(format!(
                "Derived fixture schema differs for {table}: main={main_columns:?}, frozen={frozen_columns:?}"
            ));
        }
    }

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("Begin derived model fixture extraction: {error}"))?;
    let copied_sources = tx
        .execute(
            "INSERT INTO main.model_observation_sources
             SELECT source.*
             FROM frozen.model_observation_sources AS source
             WHERE EXISTS (
                 SELECT 1
                 FROM frozen.model_usage_observations AS observation
                 WHERE observation.provider = source.provider
                   AND observation.source_key = source.source_key
                   AND observation.observed_at_ms >= ?1
                   AND observation.observed_at_ms < ?2
             )",
            rusqlite::params![range_start_ms, range_end_ms],
        )
        .map_err(|error| format!("Copy derived model source registry: {error}"))?;
    let copied_observations = tx
        .execute(
            "INSERT INTO main.model_usage_observations
             SELECT observation.*
             FROM frozen.model_usage_observations AS observation
             WHERE observation.observed_at_ms >= ?1
               AND observation.observed_at_ms < ?2",
            rusqlite::params![range_start_ms, range_end_ms],
        )
        .map_err(|error| format!("Copy derived model observations: {error}"))?;
    tx.commit()
        .map_err(|error| format!("Commit derived model fixture extraction: {error}"))?;
    conn.execute_batch("DETACH DATABASE frozen; PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|error| format!("Finalize derived model fixture extraction: {error}"))?;
    drop(conn);

    let fixture_bytes_before_backfill = fixture
        .metadata()
        .map_err(|error| format!("Read derived model fixture metadata: {error}"))?
        .len();
    let consistency = verify_model_rollup_copy(fixture, pinned_end)?;
    Ok(ModelRollupDerivedCorpusReport {
        source_path: source.display().to_string(),
        source_bytes: source_metadata.len(),
        source_read_only: source_metadata.permissions().readonly(),
        range_start: range_start.to_rfc3339(),
        range_end: pinned_end.to_rfc3339(),
        copied_sources: u64::try_from(copied_sources)
            .map_err(|_| "Copied model source count exceeds u64".to_string())?,
        copied_observations: u64::try_from(copied_observations)
            .map_err(|_| "Copied model observation count exceeds u64".to_string())?,
        available_bytes_before: available_bytes,
        required_bytes,
        fixture_bytes_before_backfill,
        consistency,
    })
}

fn runtime_copy_boundary(
    conn: &rusqlite::Connection,
    provider: &str,
    source_key: &str,
    copied_start_ms: i64,
    range_end: &str,
) -> Result<Option<(String, i64)>, String> {
    let mut statement = conn
        .prepare(
            "SELECT rowid, timestamp, kind
             FROM frozen.session_events
             WHERE provider = ?1 AND source_key = ?2 AND timestamp < ?3
             ORDER BY timestamp, rowid",
        )
        .map_err(|error| format!("Prepare runtime boundary scan: {error}"))?;
    let rows = statement
        .query_map(rusqlite::params![provider, source_key, range_end], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("Query runtime boundary scan: {error}"))?;
    let mut turn_start: Option<(String, i64)> = None;
    let mut previous: Option<(i64, String)> = None;
    for row in rows {
        let (rowid, timestamp, kind) =
            row.map_err(|error| format!("Read runtime boundary event: {error}"))?;
        let Ok(parsed) = DateTime::parse_from_rfc3339(&timestamp) else {
            continue;
        };
        let timestamp_ms = parsed.timestamp_millis();
        if timestamp_ms < 0 {
            return Err(format!(
                "Runtime boundary event {provider}/{source_key}/{rowid} predates Unix epoch"
            ));
        }
        if let Some((previous_ms, previous_kind)) = &previous {
            let gap_ms = timestamp_ms.saturating_sub(*previous_ms);
            if !runtime_gap_continues(previous_kind, &kind, gap_ms) {
                turn_start = Some((timestamp.clone(), rowid));
            }
        } else {
            turn_start = Some((timestamp.clone(), rowid));
        }
        previous = Some((timestamp_ms, kind));
        if timestamp_ms >= copied_start_ms {
            return Ok(turn_start);
        }
    }
    Ok(None)
}

/// Derive a bounded runtime fixture and compare an independent raw reference
/// with the unchanged production backfill and completed hybrid reader.
// @lat: [[runtime-rollup-tests#Runtime Rollup Test Specs#Frozen Corpus Independent Runtime Parity]]
pub fn verify_runtime_parity_from_frozen(
    source: &Path,
    fixture: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<RuntimeParityDerivedCorpusReport, String> {
    let source = source
        .canonicalize()
        .map_err(|error| format!("Resolve frozen runtime corpus: {error}"))?;
    let source_metadata = source
        .metadata()
        .map_err(|error| format!("Read frozen runtime corpus metadata: {error}"))?;
    if !source_metadata.is_file() || !source_metadata.permissions().readonly() {
        return Err(format!(
            "Frozen runtime corpus must be a read-only file: {}",
            source.display()
        ));
    }
    if fixture.exists() {
        return Err(format!(
            "Refusing to overwrite derived runtime fixture: {}",
            fixture.display()
        ));
    }
    let fixture_parent = fixture
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "Derived runtime fixture must have a parent directory".to_string())?;

    let range_start = pinned_end - TimeDelta::days(90);
    let copied_start_ms = range_start
        .timestamp_millis()
        .div_euclid(RUNTIME_REFERENCE_HOUR_MS)
        * RUNTIME_REFERENCE_HOUR_MS;
    let copied_start = DateTime::<Utc>::from_timestamp_millis(copied_start_ms)
        .ok_or_else(|| "Derived runtime copied start is not representable".to_string())?;
    let copied_start_text = copied_start.to_rfc3339();
    let range_end_text = pinned_end.to_rfc3339();

    let storage = Storage::init_study_scratch(fixture)?;
    drop(storage);
    let mut conn = rusqlite::Connection::open(fixture)
        .map_err(|error| format!("Open derived runtime fixture: {error}"))?;
    let source_path = source
        .to_str()
        .ok_or_else(|| "Frozen runtime corpus path is not UTF-8".to_string())?;
    if source_path.contains(['?', '#']) {
        return Err("Frozen runtime corpus path cannot contain URI query delimiters".to_string());
    }
    let source_uri = format!("file:{source_path}?mode=ro&immutable=1");
    conn.execute("ATTACH DATABASE ?1 AS frozen", [&source_uri])
        .map_err(|error| format!("Attach immutable runtime corpus: {error}"))?;
    for table in ["transcript_analytics_sources", "session_events"] {
        let main_columns = attached_table_columns(&conn, "main", table)?;
        let frozen_columns = attached_table_columns(&conn, "frozen", table)?;
        if main_columns != frozen_columns {
            return Err(format!(
                "Derived runtime fixture schema differs for {table}: main={main_columns:?}, frozen={frozen_columns:?}"
            ));
        }
    }

    conn.execute_batch(
        "CREATE TEMP TABLE runtime_fixture_sources (
             provider        TEXT NOT NULL,
             source_key      TEXT NOT NULL,
             start_timestamp TEXT NOT NULL,
             start_rowid     INTEGER NOT NULL,
             PRIMARY KEY(provider, source_key)
         ) WITHOUT ROWID;",
    )
    .map_err(|error| format!("Create runtime fixture source frontier: {error}"))?;
    let source_keys = {
        let mut statement = conn
            .prepare(
                "SELECT source.provider, source.source_key
                 FROM frozen.transcript_analytics_sources AS source
                 WHERE source.processing_status != 'suppressed'
                   AND source.suppressed_sha256 IS NULL
                   AND source.analytics_session_id IS NOT NULL
                   AND source.chain_id IS NOT NULL
                   AND EXISTS (
                       SELECT 1 FROM frozen.session_events AS event
                       WHERE event.provider = source.provider
                         AND event.source_key = source.source_key
                         AND event.timestamp >= ?1 AND event.timestamp < ?2
                   )
                 ORDER BY source.provider, source.source_key",
            )
            .map_err(|error| format!("Prepare runtime fixture source scan: {error}"))?;
        statement
            .query_map(
                rusqlite::params![copied_start_text, range_end_text],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| format!("Query runtime fixture sources: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Read runtime fixture sources: {error}"))?
    };
    for (provider, source_key) in &source_keys {
        let Some((start_timestamp, start_rowid)) = runtime_copy_boundary(
            &conn,
            provider,
            source_key,
            copied_start_ms,
            &range_end_text,
        )?
        else {
            continue;
        };
        conn.execute(
            "INSERT INTO runtime_fixture_sources (
                 provider, source_key, start_timestamp, start_rowid
             ) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![provider, source_key, start_timestamp, start_rowid],
        )
        .map_err(|error| format!("Record runtime fixture source frontier: {error}"))?;
    }
    let copied_sources = conn
        .query_row("SELECT COUNT(*) FROM runtime_fixture_sources", [], |row| {
            row.get::<_, u64>(0)
        })
        .map_err(|error| format!("Count runtime fixture sources: {error}"))?;
    let copied_events = conn
        .query_row(
            "SELECT COUNT(*)
             FROM frozen.session_events AS event
             JOIN runtime_fixture_sources AS selected
               ON selected.provider = event.provider
              AND selected.source_key = event.source_key
             WHERE event.timestamp < ?1
               AND (event.timestamp > selected.start_timestamp
                    OR (event.timestamp = selected.start_timestamp
                        AND event.rowid >= selected.start_rowid))",
            [&range_end_text],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| format!("Count derived runtime events: {error}"))?;
    let available_bytes = crate::retention_engine::available_disk_space(fixture_parent)?;
    let required_bytes = copied_events
        .saturating_mul(512)
        .saturating_add(copied_sources.saturating_mul(4_096))
        .saturating_mul(2);
    if available_bytes < required_bytes {
        return Err(format!(
            "Derived runtime fixture needs an estimated {required_bytes} free bytes; found {available_bytes}"
        ));
    }

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("Begin derived runtime fixture extraction: {error}"))?;
    let inserted_sources = tx
        .execute(
            "INSERT INTO main.transcript_analytics_sources
             SELECT source.*
             FROM frozen.transcript_analytics_sources AS source
             JOIN runtime_fixture_sources AS selected
               ON selected.provider = source.provider
              AND selected.source_key = source.source_key",
            [],
        )
        .map_err(|error| format!("Copy derived runtime source registry: {error}"))?;
    let inserted_events = tx
        .execute(
            "INSERT INTO main.session_events
             SELECT event.*
             FROM frozen.session_events AS event
             JOIN runtime_fixture_sources AS selected
               ON selected.provider = event.provider
              AND selected.source_key = event.source_key
             WHERE event.timestamp < ?1
               AND (event.timestamp > selected.start_timestamp
                    OR (event.timestamp = selected.start_timestamp
                        AND event.rowid >= selected.start_rowid))
             ORDER BY event.provider, event.source_key, event.timestamp, event.rowid",
            [&range_end_text],
        )
        .map_err(|error| format!("Copy derived runtime events: {error}"))?;
    if inserted_sources as u64 != copied_sources || inserted_events as u64 != copied_events {
        return Err(format!(
            "Derived runtime copy counts changed: sources {inserted_sources}/{copied_sources}, events {inserted_events}/{copied_events}"
        ));
    }
    tx.commit()
        .map_err(|error| format!("Commit derived runtime fixture extraction: {error}"))?;
    conn.execute_batch("DETACH DATABASE frozen; PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|error| format!("Finalize derived runtime fixture extraction: {error}"))?;
    drop(conn);

    let fixture_bytes_before_backfill = fixture
        .metadata()
        .map_err(|error| format!("Read derived runtime fixture metadata: {error}"))?
        .len();
    let storage = Storage::init_study_scratch(fixture)?;
    let reference_sources = {
        let conn = rusqlite::Connection::open_with_flags(
            fixture,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("Open independent runtime reference reader: {error}"))?;
        load_runtime_reference_sources(&conn)?
    };

    let chunks = std::cell::Cell::new(0_u64);
    let progress = |_: &RollupBackfillProgress| chunks.set(chunks.get() + 1);
    let controls = RollupBackfillControls {
        progress: Some(&progress),
        ..RollupBackfillControls::default()
    };
    let backfill_started = Instant::now();
    let backfill = storage.run_runtime_rollup_backfill_with_controls(&controls)?;
    let backfill_elapsed_ms = backfill_started.elapsed().as_secs_f64() * 1_000.0;
    if backfill.terminal != RollupBackfillTerminal::Completed {
        return Err(format!(
            "Derived runtime backfill did not complete: {:?}",
            backfill.terminal
        ));
    }

    let mut equality = Vec::with_capacity(WINDOWS.len() * 2);
    for window in WINDOWS {
        for (scope, parent_only) in [("all", false), ("parent_only", true)] {
            let reference = independent_runtime_reference(
                &reference_sources,
                window.duration,
                pinned_end,
                parent_only,
            );
            let production = with_pinned_query_now(pinned_end, || {
                storage.get_llm_runtime_stats(window.label, parent_only.then_some("parent_only"))
            })?;
            let repeated = with_pinned_query_now(pinned_end, || {
                storage.get_llm_runtime_stats(window.label, parent_only.then_some("parent_only"))
            })?;
            let (normalized_bytes, normalized_sha256) =
                require_runtime_parity(&reference, &production, window.label, scope)?;
            if canonical_runtime_stats(&production)? != canonical_runtime_stats(&repeated)? {
                return Err(format!(
                    "Completed runtime output changed across repeated reads at {}/{scope}",
                    window.label
                ));
            }
            equality.push(RuntimeParityWindowReport {
                window: window.label,
                scope,
                total_runtime_secs: production.total_runtime_secs,
                turn_count: production.turn_count,
                session_count: production.session_count,
                avg_per_turn_secs: production.avg_per_turn_secs,
                sparkline: production.sparkline,
                normalized_bytes,
                normalized_sha256,
                exact: true,
                repeated_stable: true,
            });
        }
    }

    let conn = rusqlite::Connection::open(fixture)
        .map_err(|error| format!("Open derived runtime fixture for validation: {error}"))?;
    let quick_check = conn
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Validate derived runtime fixture: {error}"))?;
    if quick_check != "ok" {
        return Err(format!(
            "Derived runtime fixture quick_check failed: {quick_check}"
        ));
    }
    let runtime_rollup_rows = conn
        .query_row("SELECT COUNT(*) FROM runtime_hourly", [], |row| {
            row.get::<_, u64>(0)
        })
        .map_err(|error| format!("Count derived runtime rollups: {error}"))?;
    let runtime_state_rows = conn
        .query_row("SELECT COUNT(*) FROM runtime_turn_state", [], |row| {
            row.get::<_, u64>(0)
        })
        .map_err(|error| format!("Count derived runtime states: {error}"))?;

    Ok(RuntimeParityDerivedCorpusReport {
        source_path: source.display().to_string(),
        source_bytes: source_metadata.len(),
        source_read_only: source_metadata.permissions().readonly(),
        range_start: range_start.to_rfc3339(),
        copied_start: copied_start.to_rfc3339(),
        range_end: pinned_end.to_rfc3339(),
        copied_sources,
        copied_events,
        available_bytes_before: available_bytes,
        required_bytes,
        fixture_bytes_before_backfill,
        backfill_rows_done: backfill.progress.rows_done,
        backfill_rows_total: backfill.progress.rows_total,
        backfill_chunks: chunks.get(),
        backfill_elapsed_ms,
        runtime_rollup_rows,
        runtime_state_rows,
        quick_check,
        equality,
    })
}

/// Measure only the 90-day runtime acceptance query on a prepared copy.
pub fn measure_runtime_90d(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<WidgetQueryMeasurement, String> {
    measure(
        corpus,
        pinned_end,
        WINDOWS[2],
        "get_llm_runtime_stats",
        |storage| storage.get_llm_runtime_stats("90d", None),
    )
}

fn planner_stats_snapshot(corpus: &Path) -> Result<QueryPlannerStatsSnapshot, String> {
    let connection = rusqlite::Connection::open_with_flags(
        corpus,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Open query-plan audit statistics reader: {error}"))?;
    let exists = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_schema
                 WHERE type = 'table' AND name = 'sqlite_stat1'
             )",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("Inspect query-plan audit sqlite_stat1: {error}"))?
        == 1;
    if !exists {
        return Ok(QueryPlannerStatsSnapshot {
            exists: false,
            rows: 0,
            sha256: None,
        });
    }

    let mut statement = connection
        .prepare("SELECT tbl, idx, stat FROM sqlite_stat1 ORDER BY tbl, idx, stat")
        .map_err(|error| format!("Prepare query-plan audit statistics snapshot: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("Read query-plan audit statistics snapshot: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("Materialize query-plan audit statistics: {error}"))?;
    let encoded = serde_json::to_vec(&rows)
        .map_err(|error| format!("Serialize query-plan audit statistics: {error}"))?;
    let row_count = i64::try_from(rows.len())
        .map_err(|_| "sqlite_stat1 row count exceeds SQLite INTEGER range".to_string())?;
    Ok(QueryPlannerStatsSnapshot {
        exists: true,
        rows: row_count,
        sha256: Some(format!("{:x}", Sha256::digest(encoded))),
    })
}

fn capture_widget_query_matrix(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<(WidgetQueryBenchmarkReport, Vec<WidgetQueryTraceStatement>), String> {
    begin_widget_query_trace()?;
    let benchmark = run_widget_query_baseline_inner(corpus, pinned_end, false);
    let trace = finish_widget_query_trace();
    match (benchmark, trace) {
        (Ok(benchmark), Ok(trace)) => Ok((benchmark, trace)),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
        (Err(benchmark), Err(trace)) => Err(format!(
            "Widget query audit failed ({benchmark}); trace cleanup also failed ({trace})"
        )),
    }
}

#[derive(Debug)]
struct CapturedQueryPlan {
    path: String,
    connection_id: u64,
    sequence: usize,
    sql_shape_sha256: String,
    sql_shape: String,
    expanded_sha256: String,
    plan: Vec<String>,
}

fn leading_sql_keyword(sql: &str) -> String {
    sql.trim_start()
        .split_once(char::is_whitespace)
        .map_or_else(
            || sql.trim().to_string(),
            |(keyword, _)| keyword.to_string(),
        )
        .to_ascii_uppercase()
}

fn normalized_sql_shape(sql: &str) -> String {
    let chars = sql.chars().collect::<Vec<_>>();
    let mut normalized = String::with_capacity(sql.len());
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character == '\'' {
            normalized.push('?');
            index += 1;
            while index < chars.len() {
                if chars[index] == '\'' {
                    if index + 1 < chars.len() && chars[index + 1] == '\'' {
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }
        let previous_is_identifier =
            index > 0 && (chars[index - 1].is_ascii_alphanumeric() || chars[index - 1] == '_');
        if character.is_ascii_digit() && !previous_is_identifier {
            normalized.push('?');
            index += 1;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric()
                    || matches!(chars[index], '.' | '_' | '+' | '-'))
            {
                index += 1;
            }
            continue;
        }
        normalized.push(character);
        index += 1;
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn statement_has_query_plan(keyword: &str) -> bool {
    matches!(
        keyword,
        "SELECT" | "WITH" | "CREATE" | "INSERT" | "UPDATE" | "DELETE"
    )
}

fn statement_changes_replay_state(keyword: &str) -> bool {
    matches!(
        keyword,
        "BEGIN"
            | "COMMIT"
            | "ROLLBACK"
            | "SAVEPOINT"
            | "RELEASE"
            | "CREATE"
            | "DROP"
            | "INSERT"
            | "UPDATE"
            | "DELETE"
    )
}

fn explain_production_trace(
    corpus: &Path,
    trace: Vec<WidgetQueryTraceStatement>,
) -> Result<Vec<CapturedQueryPlan>, String> {
    let mut groups = BTreeMap::<(String, u64), Vec<String>>::new();
    for statement in trace {
        if statement.path.ends_with("/cold") {
            groups
                .entry((statement.path, statement.connection_id))
                .or_default()
                .push(statement.sql);
        }
    }

    let mut captured = Vec::new();
    for ((path, connection_id), statements) in groups {
        let connection = rusqlite::Connection::open_with_flags(
            corpus,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("Open query-plan replay connection for {path}: {error}"))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| format!("Configure query-plan replay timeout for {path}: {error}"))?;
        connection
            .execute_batch(
                "PRAGMA temp_store = MEMORY;
                 PRAGMA mmap_size = 268435456;
                 PRAGMA cache_size = -65536;",
            )
            .map_err(|error| {
                format!("Configure query-plan replay connection for {path}: {error}")
            })?;

        for (sequence, sql) in statements.into_iter().enumerate() {
            let keyword = leading_sql_keyword(&sql);
            if statement_has_query_plan(&keyword) {
                let mut statement = connection
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .map_err(|error| {
                        format!(
                            "Prepare traced query plan for {path} connection {connection_id} statement {sequence}: {error}"
                        )
                    })?;
                let plan = statement
                    .query_map([], |row| row.get::<_, String>(3))
                    .map_err(|error| {
                        format!(
                            "Execute traced query plan for {path} connection {connection_id} statement {sequence}: {error}"
                        )
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|error| {
                        format!(
                            "Read traced query plan for {path} connection {connection_id} statement {sequence}: {error}"
                        )
                    })?;
                if !plan.is_empty() {
                    let sql_shape = normalized_sql_shape(&sql);
                    captured.push(CapturedQueryPlan {
                        path: path.clone(),
                        connection_id,
                        sequence,
                        sql_shape_sha256: format!("{:x}", Sha256::digest(sql_shape.as_bytes())),
                        sql_shape,
                        expanded_sha256: format!("{:x}", Sha256::digest(sql.as_bytes())),
                        plan,
                    });
                }
            }
            if statement_changes_replay_state(&keyword) {
                connection.execute_batch(&sql).map_err(|error| {
                    format!(
                        "Replay traced state for {path} connection {connection_id} statement {sequence}: {error}"
                    )
                })?;
            }
        }
    }
    Ok(captured)
}

fn plan_index_names(plan: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for detail in plan {
        for marker in ["USING COVERING INDEX ", "USING INDEX "] {
            let Some((_, suffix)) = detail.split_once(marker) else {
                continue;
            };
            let name = suffix
                .split(|character: char| character.is_whitespace() || character == '(')
                .next()
                .unwrap_or_default();
            if !name.is_empty() && !names.iter().any(|candidate| candidate == name) {
                names.push(name.to_string());
            }
        }
    }
    names
}

fn plan_regression_reasons(before: &[String], after: &[String]) -> Vec<String> {
    let count = |plan: &[String], marker: &str| {
        plan.iter().filter(|detail| detail.contains(marker)).count()
    };
    let mut reasons = Vec::new();
    let before_scans = count(before, "SCAN ");
    let after_scans = count(after, "SCAN ");
    if after_scans > before_scans {
        reasons.push(format!(
            "SCAN count increased from {before_scans} to {after_scans}"
        ));
    }
    let before_searches = count(before, "SEARCH ");
    let after_searches = count(after, "SEARCH ");
    if after_searches < before_searches {
        reasons.push(format!(
            "SEARCH count decreased from {before_searches} to {after_searches}"
        ));
    }
    let before_temp_order = count(before, "USE TEMP B-TREE");
    let after_temp_order = count(after, "USE TEMP B-TREE");
    if after_temp_order > before_temp_order {
        reasons.push(format!(
            "temporary ordering structures increased from {before_temp_order} to {after_temp_order}"
        ));
    }
    let after_text = after.join("\n");
    for index in plan_index_names(before) {
        if !after_text.contains(&index) {
            reasons.push(format!("planner stopped using index {index}"));
        }
    }
    reasons
}

fn compare_captured_plans(
    before: Vec<CapturedQueryPlan>,
    after: Vec<CapturedQueryPlan>,
) -> Result<Vec<WidgetQueryPlanAuditEntry>, String> {
    let group = |plans: Vec<CapturedQueryPlan>| {
        let mut grouped = BTreeMap::<(String, String), Vec<CapturedQueryPlan>>::new();
        for plan in plans {
            grouped
                .entry((plan.path.clone(), plan.sql_shape_sha256.clone()))
                .or_default()
                .push(plan);
        }
        grouped
    };
    let before = group(before);
    let mut after = group(after);
    if before.keys().ne(after.keys()) {
        let before_keys = before.keys().cloned().collect::<Vec<_>>();
        let after_keys = after.keys().cloned().collect::<Vec<_>>();
        return Err(format!(
            "Traced production SQL set changed after ANALYZE: before={before_keys:?}, after={after_keys:?}"
        ));
    }

    let mut compared = Vec::new();
    for (key, before_group) in before {
        let after_group = after
            .remove(&key)
            .expect("matching key set was checked above");
        if before_group.len() != after_group.len() {
            return Err(format!(
                "Traced production SQL occurrence count changed after ANALYZE for {}/{}: before={}, after={}",
                key.0,
                key.1,
                before_group.len(),
                after_group.len()
            ));
        }
        for (before, after) in before_group.into_iter().zip(after_group) {
            if before.sql_shape != after.sql_shape {
                return Err(format!(
                    "Traced production SQL shape digest collision at {}/{}",
                    before.path, before.sql_shape_sha256
                ));
            }
            let regression_reasons = plan_regression_reasons(&before.plan, &after.plan);
            compared.push(WidgetQueryPlanAuditEntry {
                path: before.path,
                connection_id: before.connection_id,
                sequence: before.sequence,
                sql_shape_sha256: before.sql_shape_sha256,
                sql_shape: before.sql_shape,
                before_expanded_sha256: before.expanded_sha256,
                after_expanded_sha256: after.expanded_sha256,
                plan_changed: before.plan != after.plan,
                before_plan: before.plan,
                after_plan: after.plan,
                regression_reasons,
            });
        }
    }
    Ok(compared)
}

fn timing_comparisons(
    before: &WidgetQueryBenchmarkReport,
    after: &WidgetQueryBenchmarkReport,
) -> Result<Vec<WidgetQueryTimingComparison>, String> {
    let mut before_rows = BTreeMap::<String, (f64, f64, usize)>::new();
    for measurement in &before.measurements {
        before_rows.insert(
            format!("query/{}/{}", measurement.window, measurement.query),
            (
                measurement.elapsed_ms,
                measurement.warm_elapsed_ms,
                measurement.output_bytes,
            ),
        );
    }
    for measurement in &before.view_fanouts {
        before_rows.insert(
            format!("fanout/{}", measurement.view),
            (
                measurement.cold_elapsed_ms,
                measurement.warm_elapsed_ms,
                measurement.output_bytes,
            ),
        );
    }

    let mut after_rows = BTreeMap::<String, (f64, f64, usize)>::new();
    for measurement in &after.measurements {
        after_rows.insert(
            format!("query/{}/{}", measurement.window, measurement.query),
            (
                measurement.elapsed_ms,
                measurement.warm_elapsed_ms,
                measurement.output_bytes,
            ),
        );
    }
    for measurement in &after.view_fanouts {
        after_rows.insert(
            format!("fanout/{}", measurement.view),
            (
                measurement.cold_elapsed_ms,
                measurement.warm_elapsed_ms,
                measurement.output_bytes,
            ),
        );
    }
    if before_rows.keys().ne(after_rows.keys()) {
        return Err("Timed production path set changed after ANALYZE".to_string());
    }

    before_rows
        .into_iter()
        .map(|(path, (before_cold_ms, before_warm_ms, before_bytes))| {
            let (after_cold_ms, after_warm_ms, after_bytes) = after_rows[&path];
            if before_bytes != after_bytes {
                return Err(format!(
                    "Serialized output size changed after ANALYZE for {path}: before={before_bytes}, after={after_bytes}"
                ));
            }
            let cold_delta_ms = after_cold_ms - before_cold_ms;
            let cold_ratio = if before_cold_ms > 0.0 {
                after_cold_ms / before_cold_ms
            } else if after_cold_ms == 0.0 {
                1.0
            } else {
                f64::INFINITY
            };
            // Ignore sub-5 ms scheduler noise; above that floor, a 25% cold
            // slowdown is material enough to block a planner-statistics change.
            let material_regression = cold_delta_ms > 5.0 && cold_ratio > 1.25;
            Ok(WidgetQueryTimingComparison {
                path,
                before_cold_ms,
                after_cold_ms,
                before_warm_ms,
                after_warm_ms,
                cold_delta_ms,
                cold_ratio,
                material_regression,
            })
        })
        .collect()
}

/// Audit bounded ANALYZE against every production SQL statement exercised by
/// the feature-020 endpoint and view-fanout matrix.
// @lat: [[backend#Database#Database compaction#Bounded Query Planner Analysis]]
pub fn audit_widget_query_plans(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<WidgetQueryPlanAuditReport, String> {
    let canonical = corpus
        .canonicalize()
        .map_err(|error| format!("Resolve writable query-plan audit copy: {error}"))?;
    let before_metadata = canonical
        .metadata()
        .map_err(|error| format!("Read writable query-plan audit copy metadata: {error}"))?;
    if !before_metadata.is_file() {
        return Err(format!(
            "Query-plan audit copy is not a file: {}",
            canonical.display()
        ));
    }
    if before_metadata.permissions().readonly() {
        return Err(format!(
            "Query-plan audit requires a disposable writable copy: {}",
            canonical.display()
        ));
    }

    let before_stats = planner_stats_snapshot(&canonical)?;
    if before_stats.rows != 0 {
        return Err(format!(
            "Query-plan audit copy already contains {} sqlite_stat1 rows",
            before_stats.rows
        ));
    }
    let (before_benchmark, before_trace) = capture_widget_query_matrix(&canonical, pinned_end)?;
    let before_plans = explain_production_trace(&canonical, before_trace)?;

    let analysis_storage = Storage::init_widget_query_maintenance_audit(&canonical)?;
    let analysis = analysis_storage.run_bounded_database_analysis()?;
    drop(analysis_storage);
    let after_stats = planner_stats_snapshot(&canonical)?;
    if !after_stats.exists || after_stats.rows <= 0 {
        return Err(format!(
            "Bounded ANALYZE did not populate sqlite_stat1: exists={}, rows={}",
            after_stats.exists, after_stats.rows
        ));
    }

    let (after_benchmark, after_trace) = capture_widget_query_matrix(&canonical, pinned_end)?;
    let after_plans = explain_production_trace(&canonical, after_trace)?;
    let plans = compare_captured_plans(before_plans, after_plans)?;
    let timings = timing_comparisons(&before_benchmark, &after_benchmark)?;
    let changed_plans = plans.iter().filter(|entry| entry.plan_changed).count();
    let plan_regressions = plans
        .iter()
        .filter(|entry| !entry.regression_reasons.is_empty())
        .count();
    let timing_regressions = timings
        .iter()
        .filter(|timing| timing.material_regression)
        .count();

    let validation = rusqlite::Connection::open_with_flags(
        &canonical,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Open analyzed audit copy for validation: {error}"))?;
    let quick_check = validation
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Validate analyzed query-plan audit copy: {error}"))?;
    if quick_check != "ok" {
        return Err(format!(
            "Analyzed query-plan audit copy quick_check failed: {quick_check}"
        ));
    }
    let after_metadata = canonical
        .metadata()
        .map_err(|error| format!("Read analyzed query-plan audit copy metadata: {error}"))?;
    let verdict = if plan_regressions == 0 && timing_regressions == 0 {
        "pass"
    } else {
        "fail"
    };

    Ok(WidgetQueryPlanAuditReport {
        corpus_path: canonical.display().to_string(),
        corpus_bytes_before: before_metadata.len(),
        corpus_bytes_after: after_metadata.len(),
        pinned_end: pinned_end.to_rfc3339(),
        sqlite_version: rusqlite::version().to_string(),
        before_stats,
        analysis,
        after_stats,
        audited_sql_statements: plans.len(),
        changed_plans,
        plan_regressions,
        timing_regressions,
        plans,
        timings,
        before_benchmark,
        after_benchmark,
        quick_check,
        verdict,
    })
}

/// Remove planner statistics from a disposable audit copy, reload statless
/// planner state, and truncate its WAL.
pub fn clear_query_planner_stats_for_audit(corpus: &Path) -> Result<(), String> {
    let mut connection = rusqlite::Connection::open_with_flags(
        corpus,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Open focused A/B statistics writer: {error}"))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("Configure focused A/B statistics timeout: {error}"))?;
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("Begin focused A/B statistics clear: {error}"))?;
    tx.execute("DELETE FROM sqlite_stat1", [])
        .map_err(|error| format!("Clear focused A/B sqlite_stat1 rows: {error}"))?;
    tx.commit()
        .map_err(|error| format!("Commit focused A/B statistics clear: {error}"))?;
    connection
        .execute_batch("ANALYZE sqlite_schema;")
        .map_err(|error| format!("Reload statless focused A/B planner state: {error}"))?;
    let (busy, log_frames, checkpointed_frames) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| format!("Checkpoint focused A/B statistics clear: {error}"))?;
    if busy != 0 || log_frames != checkpointed_frames {
        return Err(format!(
            "Focused A/B statless checkpoint incomplete: busy={busy}, log_frames={log_frames}, checkpointed_frames={checkpointed_frames}"
        ));
    }
    let remaining = connection
        .query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| format!("Verify focused A/B statless planner state: {error}"))?;
    if remaining != 0 {
        return Err(format!(
            "Focused A/B statistics clear left {remaining} sqlite_stat1 rows"
        ));
    }
    Ok(())
}

fn measure_model_history_ab_sample(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    state: &'static str,
    ordinal: usize,
) -> Result<(ModelHistoryAbSample, Vec<WidgetQueryTraceStatement>), String> {
    begin_widget_query_trace()?;
    let measurement = (|| -> Result<ModelHistoryAbSample, String> {
        let storage = Storage::init_widget_query_benchmark(corpus)?;
        with_pinned_query_now(pinned_end, || {
            set_widget_query_trace_path("focused/model_history_24h/cold");
            let cold_started = Instant::now();
            let cold = storage.get_model_history(ModelRange::TwentyFourHours, None, None)?;
            let cold_ms = cold_started.elapsed().as_secs_f64() * 1_000.0;
            let cold = serde_json::to_vec(&cold)
                .map_err(|error| format!("Serialize focused cold model history: {error}"))?;

            set_widget_query_trace_path("focused/model_history_24h/warm");
            let warm_started = Instant::now();
            let warm = storage.get_model_history(ModelRange::TwentyFourHours, None, None)?;
            let warm_ms = warm_started.elapsed().as_secs_f64() * 1_000.0;
            let warm = serde_json::to_vec(&warm)
                .map_err(|error| format!("Serialize focused warm model history: {error}"))?;
            if cold != warm {
                return Err(format!(
                    "Focused {state} model-history cold and warm outputs differ"
                ));
            }
            Ok(ModelHistoryAbSample {
                state,
                ordinal,
                cold_ms,
                warm_ms,
                output_bytes: cold.len(),
                output_sha256: format!("{:x}", Sha256::digest(&cold)),
            })
        })
    })();
    let trace = finish_widget_query_trace();
    match (measurement, trace) {
        (Ok(measurement), Ok(trace)) => Ok((measurement, trace)),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
        (Err(measurement), Err(trace)) => Err(format!(
            "Focused model-history A/B failed ({measurement}); trace cleanup also failed ({trace})"
        )),
    }
}

fn median(values: impl IntoIterator<Item = f64>) -> Result<f64, String> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        return Err("Cannot compute median of an empty sample set".to_string());
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Ok(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

/// Recheck a flagged model-history timing with repeated statless/analyzed
/// pairs on the same disposable copy.
pub fn audit_model_history_24h_ab(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    samples_per_state: usize,
) -> Result<ModelHistoryAbReport, String> {
    let samples_per_state = samples_per_state.clamp(2, 20);
    let canonical = corpus
        .canonicalize()
        .map_err(|error| format!("Resolve focused model-history A/B copy: {error}"))?;
    let initial_stats = planner_stats_snapshot(&canonical)?;
    if !initial_stats.exists || initial_stats.rows <= 0 {
        return Err("Focused model-history A/B requires the analyzed audit copy".to_string());
    }

    let mut statless_samples = Vec::with_capacity(samples_per_state);
    let mut analyzed_samples = Vec::with_capacity(samples_per_state);
    let mut statless_plan = None;
    let mut analyzed_plan = None;
    let mut final_analysis = None;
    for ordinal in 0..samples_per_state {
        clear_query_planner_stats_for_audit(&canonical)?;
        let (statless, trace) =
            measure_model_history_ab_sample(&canonical, pinned_end, "statless", ordinal)?;
        if statless_plan.is_none() {
            statless_plan = Some(explain_production_trace(&canonical, trace)?);
        }
        statless_samples.push(statless);

        let storage = Storage::init_widget_query_maintenance_audit(&canonical)?;
        final_analysis = Some(storage.run_bounded_database_analysis()?);
        drop(storage);
        let (analyzed, trace) =
            measure_model_history_ab_sample(&canonical, pinned_end, "analyzed", ordinal)?;
        if analyzed_plan.is_none() {
            analyzed_plan = Some(explain_production_trace(&canonical, trace)?);
        }
        analyzed_samples.push(analyzed);
    }

    let mut output_hashes = statless_samples
        .iter()
        .chain(&analyzed_samples)
        .map(|sample| (sample.output_bytes, sample.output_sha256.as_str()));
    let Some(expected_output) = output_hashes.next() else {
        return Err("Focused model-history A/B produced no samples".to_string());
    };
    if output_hashes.any(|output| output != expected_output) {
        return Err("Focused model-history A/B output hashes differ".to_string());
    }

    let statless_median_cold_ms = median(statless_samples.iter().map(|sample| sample.cold_ms))?;
    let analyzed_median_cold_ms = median(analyzed_samples.iter().map(|sample| sample.cold_ms))?;
    let median_delta_ms = analyzed_median_cold_ms - statless_median_cold_ms;
    let median_ratio = analyzed_median_cold_ms / statless_median_cold_ms;
    let material_regression = median_delta_ms > 5.0 && median_ratio > 1.25;
    let plan_comparison = compare_captured_plans(
        statless_plan.expect("first statless sample records a plan"),
        analyzed_plan.expect("first analyzed sample records a plan"),
    )?;
    let plan_regression = plan_comparison
        .iter()
        .any(|entry| !entry.regression_reasons.is_empty());
    let final_stats = planner_stats_snapshot(&canonical)?;
    if !final_stats.exists || final_stats.rows <= 0 {
        return Err("Focused model-history A/B did not restore sqlite_stat1".to_string());
    }
    let validation = rusqlite::Connection::open_with_flags(
        &canonical,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Open focused model-history A/B validation: {error}"))?;
    let quick_check = validation
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Validate focused model-history A/B copy: {error}"))?;
    if quick_check != "ok" {
        return Err(format!(
            "Focused model-history A/B quick_check failed: {quick_check}"
        ));
    }
    let verdict = if material_regression || plan_regression {
        "fail"
    } else {
        "pass"
    };
    Ok(ModelHistoryAbReport {
        corpus_path: canonical.display().to_string(),
        pinned_end: pinned_end.to_rfc3339(),
        samples_per_state,
        initial_stats,
        statless_samples,
        analyzed_samples,
        statless_median_cold_ms,
        analyzed_median_cold_ms,
        median_delta_ms,
        median_ratio,
        material_regression,
        plan_comparison,
        final_analysis: final_analysis.expect("sample count is clamped above zero"),
        final_stats,
        quick_check,
        verdict,
    })
}

const FLAGGED_ANALYZE_PATHS: [&str; 4] = [
    "fanout/Context",
    "query/24h/get_all_bucket_stats",
    "query/24h/get_model_usage_overview",
    "query/30d/get_all_bucket_stats",
];
const PENDING_FLAGGED_ANALYZE_PATHS: [&str; 2] = [
    "query/24h/get_all_bucket_stats",
    "query/90d/get_hook_breakdown",
];

fn timed_serialized_output<T: Serialize>(
    path: &str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<(f64, Vec<u8>), String> {
    let started = Instant::now();
    let value = operation()?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let output = serde_json::to_vec(&value)
        .map_err(|error| format!("Serialize flagged ANALYZE path {path}: {error}"))?;
    Ok((elapsed_ms, output))
}

fn timed_flagged_audit_path(
    storage: &Storage,
    path: &str,
    current_buckets: &[UsageBucket],
) -> Result<(f64, Vec<u8>), String> {
    match path {
        "fanout/Context" => timed_serialized_output(path, || {
            storage.get_context_savings_analytics("30d", Some(40))
        }),
        "query/24h/get_all_bucket_stats" => {
            timed_serialized_output(path, || storage.get_all_bucket_stats(current_buckets, 1))
        }
        "query/24h/get_model_usage_overview" => timed_serialized_output(path, || {
            storage.get_model_usage_overview(ModelRange::TwentyFourHours, None)
        }),
        "query/30d/get_all_bucket_stats" => {
            timed_serialized_output(path, || storage.get_all_bucket_stats(current_buckets, 30))
        }
        "query/90d/get_hook_breakdown" => timed_serialized_output(path, || {
            storage.get_hook_breakdown("90d", None, false, Some(100))
        }),
        unexpected => Err(format!("Unsupported flagged ANALYZE path: {unexpected}")),
    }
}

fn measure_flagged_audit_path_sample(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    path: &str,
    current_buckets: &[UsageBucket],
    state: &'static str,
    ordinal: usize,
) -> Result<(FlaggedPathAbSample, Vec<WidgetQueryTraceStatement>), String> {
    begin_widget_query_trace()?;
    let measurement = (|| -> Result<FlaggedPathAbSample, String> {
        let storage = Storage::init_widget_query_benchmark(corpus)?;
        with_pinned_query_now(pinned_end, || {
            set_widget_query_trace_path(format!("{path}/cold"));
            let (cold_ms, cold) = timed_flagged_audit_path(&storage, path, current_buckets)?;

            set_widget_query_trace_path(format!("{path}/warm"));
            let (warm_ms, warm) = timed_flagged_audit_path(&storage, path, current_buckets)?;
            if cold != warm {
                return Err(format!(
                    "Flagged {state} path {path} cold and warm outputs differ"
                ));
            }
            Ok(FlaggedPathAbSample {
                state,
                ordinal,
                cold_ms,
                warm_ms,
                output_bytes: cold.len(),
                output_sha256: format!("{:x}", Sha256::digest(&cold)),
            })
        })
    })();
    let trace = finish_widget_query_trace();
    match (measurement, trace) {
        (Ok(measurement), Ok(trace)) => Ok((measurement, trace)),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
        (Err(measurement), Err(trace)) => Err(format!(
            "Flagged path A/B failed ({measurement}); trace cleanup also failed ({trace})"
        )),
    }
}

#[derive(Default)]
struct FlaggedPathAbAccumulator {
    statless_samples: Vec<FlaggedPathAbSample>,
    analyzed_samples: Vec<FlaggedPathAbSample>,
    statless_plan: Option<Vec<CapturedQueryPlan>>,
    analyzed_plan: Option<Vec<CapturedQueryPlan>>,
}

fn audit_paths_ab(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    samples_per_state: usize,
    paths: &[&str],
) -> Result<FlaggedPathsAbReport, String> {
    let samples_per_state = samples_per_state.clamp(2, 20);
    let canonical = corpus
        .canonicalize()
        .map_err(|error| format!("Resolve flagged path A/B copy: {error}"))?;
    let initial_stats = planner_stats_snapshot(&canonical)?;
    if !initial_stats.exists || initial_stats.rows <= 0 {
        return Err("Flagged path A/B requires the analyzed audit copy".to_string());
    }
    let current_buckets = latest_usage_buckets(&canonical)?;
    let mut accumulators = BTreeMap::<String, FlaggedPathAbAccumulator>::new();
    for &path in paths {
        accumulators.insert(path.to_string(), FlaggedPathAbAccumulator::default());
    }

    let mut final_analysis = None;
    for ordinal in 0..samples_per_state {
        for &path in paths {
            clear_query_planner_stats_for_audit(&canonical)?;
            let (statless, trace) = measure_flagged_audit_path_sample(
                &canonical,
                pinned_end,
                path,
                &current_buckets,
                "statless",
                ordinal,
            )?;
            let accumulator = accumulators
                .get_mut(path)
                .expect("all flagged paths have accumulators");
            if accumulator.statless_plan.is_none() {
                accumulator.statless_plan = Some(explain_production_trace(&canonical, trace)?);
            }
            accumulator.statless_samples.push(statless);

            let storage = Storage::init_widget_query_maintenance_audit(&canonical)?;
            final_analysis = Some(storage.run_bounded_database_analysis()?);
            drop(storage);
            let (analyzed, trace) = measure_flagged_audit_path_sample(
                &canonical,
                pinned_end,
                path,
                &current_buckets,
                "analyzed",
                ordinal,
            )?;
            let accumulator = accumulators
                .get_mut(path)
                .expect("all flagged paths have accumulators");
            if accumulator.analyzed_plan.is_none() {
                accumulator.analyzed_plan = Some(explain_production_trace(&canonical, trace)?);
            }
            accumulator.analyzed_samples.push(analyzed);
        }
    }

    let mut comparisons = Vec::with_capacity(paths.len());
    for &path in paths {
        let accumulator = accumulators
            .remove(path)
            .expect("all flagged paths have completed accumulators");
        let mut outputs = accumulator
            .statless_samples
            .iter()
            .chain(&accumulator.analyzed_samples)
            .map(|sample| (sample.output_bytes, sample.output_sha256.as_str()));
        let Some(expected_output) = outputs.next() else {
            return Err(format!("Flagged path A/B produced no samples for {path}"));
        };
        if outputs.any(|output| output != expected_output) {
            return Err(format!("Flagged path A/B output hashes differ for {path}"));
        }
        let statless_median_cold_ms = median(
            accumulator
                .statless_samples
                .iter()
                .map(|sample| sample.cold_ms),
        )?;
        let analyzed_median_cold_ms = median(
            accumulator
                .analyzed_samples
                .iter()
                .map(|sample| sample.cold_ms),
        )?;
        let median_delta_ms = analyzed_median_cold_ms - statless_median_cold_ms;
        let median_ratio = analyzed_median_cold_ms / statless_median_cold_ms;
        let material_regression = median_delta_ms > 5.0 && median_ratio > 1.25;
        let plan_comparison = compare_captured_plans(
            accumulator
                .statless_plan
                .expect("first statless sample records a plan"),
            accumulator
                .analyzed_plan
                .expect("first analyzed sample records a plan"),
        )?;
        comparisons.push(FlaggedPathAbComparison {
            path: path.to_string(),
            statless_samples: accumulator.statless_samples,
            analyzed_samples: accumulator.analyzed_samples,
            statless_median_cold_ms,
            analyzed_median_cold_ms,
            median_delta_ms,
            median_ratio,
            material_regression,
            plan_comparison,
        });
    }

    let final_stats = planner_stats_snapshot(&canonical)?;
    if !final_stats.exists || final_stats.rows <= 0 {
        return Err("Flagged path A/B did not restore sqlite_stat1".to_string());
    }
    let validation = rusqlite::Connection::open_with_flags(
        &canonical,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Open flagged path A/B validation: {error}"))?;
    let quick_check = validation
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Validate flagged path A/B copy: {error}"))?;
    if quick_check != "ok" {
        return Err(format!(
            "Flagged path A/B quick_check failed: {quick_check}"
        ));
    }
    let failed = comparisons.iter().any(|comparison| {
        comparison.material_regression
            || comparison
                .plan_comparison
                .iter()
                .any(|plan| !plan.regression_reasons.is_empty())
    });
    Ok(FlaggedPathsAbReport {
        corpus_path: canonical.display().to_string(),
        pinned_end: pinned_end.to_rfc3339(),
        samples_per_state,
        initial_stats,
        comparisons,
        final_analysis: final_analysis.expect("sample count is clamped above zero"),
        final_stats,
        quick_check,
        verdict: if failed { "fail" } else { "pass" },
    })
}

/// Recheck every timing row flagged by the completed-state full audit through
/// exact-path alternating statless/analyzed pairs.
pub fn audit_flagged_paths_ab(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    samples_per_state: usize,
) -> Result<FlaggedPathsAbReport, String> {
    audit_paths_ab(
        corpus,
        pinned_end,
        samples_per_state,
        &FLAGGED_ANALYZE_PATHS,
    )
}

/// Recheck both pending-state timing flags without running completed-state
/// queries between each statless/analyzed pair.
pub fn audit_pending_flagged_paths_ab(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    samples_per_state: usize,
) -> Result<FlaggedPathsAbReport, String> {
    audit_paths_ab(
        corpus,
        pinned_end,
        samples_per_state,
        &PENDING_FLAGGED_ANALYZE_PATHS,
    )
}

/// Run the complete BEFORE query matrix against one immutable corpus.
// @lat: [[backend#Database#Widget query benchmark corpus]]
pub fn run_widget_query_baseline(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<WidgetQueryBenchmarkReport, String> {
    run_widget_query_baseline_inner(corpus, pinned_end, true)
}

fn run_widget_query_baseline_inner(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    require_read_only: bool,
) -> Result<WidgetQueryBenchmarkReport, String> {
    let canonical = corpus
        .canonicalize()
        .map_err(|error| format!("Resolve widget query benchmark corpus: {error}"))?;
    let metadata = canonical
        .metadata()
        .map_err(|error| format!("Read widget query benchmark corpus metadata: {error}"))?;
    if !metadata.is_file() {
        return Err(format!(
            "Widget query benchmark corpus is not a file: {}",
            canonical.display()
        ));
    }
    if require_read_only && !metadata.permissions().readonly() {
        return Err(format!(
            "Widget query benchmark corpus must be read-only: {}",
            canonical.display()
        ));
    }

    let inspection = rusqlite::Connection::open_with_flags(
        &canonical,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Inspect widget query benchmark corpus: {error}"))?;
    inspection
        .execute_batch("PRAGMA query_only = ON;")
        .map_err(|error| format!("Protect widget query benchmark inspection: {error}"))?;
    let schema_version = inspection
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("Read benchmark schema version: {error}"))?;
    let page_size = inspection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| format!("Read benchmark page size: {error}"))?;
    let page_count = inspection
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .map_err(|error| format!("Read benchmark page count: {error}"))?;
    let freelist_count = inspection
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .map_err(|error| format!("Read benchmark freelist count: {error}"))?;
    drop(inspection);

    let current_buckets = latest_usage_buckets(&canonical)?;
    let mut measurements = Vec::with_capacity(WINDOWS.len() * 13);
    for window in WINDOWS {
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_model_usage_overview",
            |storage| storage.get_model_usage_overview(window.model_range, None),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_model_history",
            |storage| storage.get_model_history(window.model_range, None, None),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_token_history",
            |storage| storage.get_token_history(window.label, None, None, None, None),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_llm_runtime_stats",
            |storage| storage.get_llm_runtime_stats(window.label, None),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_code_stats",
            |storage| storage.get_code_stats(window.label),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_code_stats_history",
            |storage| storage.get_code_stats_history(window.label),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_host_breakdown",
            |storage| storage.get_host_breakdown(window.label),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_project_breakdown",
            |storage| storage.get_project_breakdown(window.label),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_session_breakdown",
            |storage| storage.get_session_breakdown(window.label, None, None, Some(200)),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_skill_breakdown",
            |storage| storage.get_skill_breakdown(window.label, None, false, Some(100)),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_hook_breakdown",
            |storage| storage.get_hook_breakdown(window.label, None, false, Some(100)),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_all_bucket_stats",
            |storage| storage.get_all_bucket_stats(&current_buckets, window.bucket_days),
        )?);
        measurements.push(measure(
            &canonical,
            pinned_end,
            window,
            "get_context_savings_analytics",
            |storage| storage.get_context_savings_analytics(window.label, Some(40)),
        )?);
    }

    let view_fanouts = vec![
        measure_view_fanout(
            &canonical,
            pinned_end,
            "Usage",
            "provider series -> activity series -> token stats -> runtime -> code stats -> code history (code card) -> context savings -> retention policy -> session breakdown -> project breakdown -> token history -> code history (insights after runtime)",
            usage_view_fanout,
        )?,
        measure_view_fanout(
            &canonical,
            pinned_end,
            "Charts",
            "provider series -> code stats -> code history -> token history -> retention policy",
            charts_view_fanout,
        )?,
        measure_view_fanout(
            &canonical,
            pinned_end,
            "Context",
            "context savings",
            context_view_fanout,
        )?,
    ];

    Ok(WidgetQueryBenchmarkReport {
        corpus_path: canonical.display().to_string(),
        corpus_bytes: metadata.len(),
        corpus_read_only: metadata.permissions().readonly(),
        sqlite_version: rusqlite::version().to_string(),
        schema_version,
        page_size,
        page_count,
        freelist_count,
        pinned_end: pinned_end.to_rfc3339(),
        os_page_cache: "uncontrolled; cold means first in-process call with app caches bypassed",
        measurements,
        view_fanouts,
    })
}

#[cfg(test)]
mod runtime_reference_tests {
    use rusqlite::params;
    use serial_test::serial;
    use tempfile::TempDir;

    use super::*;

    fn seed_source(
        conn: &rusqlite::Connection,
        provider: &str,
        source_key: &str,
        session_id: &str,
        chain_id: &str,
        is_sidechain: bool,
        events: &[(DateTime<Utc>, &str)],
    ) {
        conn.execute(
            "INSERT INTO transcript_analytics_sources (
                 provider, source_key, source_root_key, source_path,
                 source_session_id, analytics_session_id, chain_id,
                 is_sidechain, content_sha256, seen_generation,
                 processing_status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, 1, 'ok')",
            params![
                provider,
                source_key,
                format!("root-{source_key}"),
                format!("/runtime-reference/{source_key}.jsonl"),
                session_id,
                chain_id,
                i64::from(is_sidechain),
                format!("sha-{source_key}"),
            ],
        )
        .expect("insert runtime reference source");
        for (ordinal, (timestamp, kind)) in events.iter().enumerate() {
            conn.execute(
                "INSERT INTO session_events (
                     provider, source_key, event_key, session_id, chain_id,
                     is_sidechain, timestamp, kind
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    provider,
                    source_key,
                    format!("{source_key}-{ordinal}"),
                    session_id,
                    chain_id,
                    i64::from(is_sidechain),
                    timestamp.to_rfc3339(),
                    kind,
                ],
            )
            .expect("insert runtime reference event");
        }
    }

    // @lat: [[runtime-rollup-tests#Runtime Rollup Test Specs#Independent Runtime Reference Edge Semantics]]
    #[test]
    #[serial]
    fn independent_runtime_reference_covers_edges_and_matches_hybrid() {
        let directory = TempDir::new().expect("runtime parity tempdir");
        let fixture = directory.path().join("runtime-parity.sqlite3");
        let storage = Storage::init_study_scratch(&fixture).expect("initialize runtime fixture");
        let conn = rusqlite::Connection::open(storage.database_path())
            .expect("open runtime fixture writer");
        let base = DateTime::parse_from_rfc3339("2026-08-02T08:00:00Z")
            .expect("parse runtime base")
            .with_timezone(&Utc);
        let at = |seconds: i64| base + TimeDelta::seconds(seconds);
        seed_source(
            &conn,
            "claude",
            "parent-source",
            "shared-session",
            "parent-chain",
            false,
            &[
                (at(0), "user_text"),
                (at(60), "asst_text"),
                (at(660), "user_text"),
                (at(720), "asst_tool_use"),
                (at(7_920), "user_tool_result"),
                (at(8_520), "user_text"),
                (at(10_500), "asst_tool_use"),
                (at(35_700), "user_tool_result"),
                (at(37_500), "asst_tool_use"),
            ],
        );
        seed_source(
            &conn,
            "claude",
            "side-source",
            "shared-session",
            "side-chain",
            true,
            &[
                (at(100), "user_text"),
                (at(220), "asst_text"),
                (at(1_000), "user_text"),
            ],
        );
        seed_source(
            &conn,
            "codex",
            "codex-source",
            "shared-session",
            "codex-chain",
            false,
            &[
                (at(200), "user_text"),
                (at(260), "asst_text"),
                (at(1_000), "user_text"),
            ],
        );
        let boundary = DateTime::parse_from_rfc3339("2026-08-01T19:10:00Z")
            .expect("parse boundary base")
            .with_timezone(&Utc);
        seed_source(
            &conn,
            "claude",
            "boundary-source",
            "boundary-session",
            "boundary-chain",
            false,
            &[
                (boundary, "user_text"),
                (boundary + TimeDelta::minutes(5), "asst_text"),
                (boundary + TimeDelta::minutes(10), "asst_text"),
                (boundary + TimeDelta::minutes(50), "user_text"),
            ],
        );
        drop(conn);

        let reference_conn = rusqlite::Connection::open_with_flags(
            storage.database_path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open independent reference reader");
        let sources = load_runtime_reference_sources(&reference_conn)
            .expect("load independent reference sources");
        drop(reference_conn);
        let pinned_now = DateTime::parse_from_rfc3339("2026-08-02T19:27:43Z")
            .expect("parse pinned runtime endpoint")
            .with_timezone(&Utc);
        let all = independent_runtime_reference(&sources, TimeDelta::hours(24), pinned_now, false);
        assert_eq!(all.total_runtime_secs, 33_463.0);
        assert_eq!(all.turn_count, 7);
        assert_eq!(all.session_count, 3);
        assert_eq!(all.sparkline.iter().sum::<f64>(), 33_463.0);
        let parent =
            independent_runtime_reference(&sources, TimeDelta::hours(24), pinned_now, true);
        assert_eq!(parent.total_runtime_secs, 33_343.0);
        assert_eq!(parent.turn_count, 6);
        assert_eq!(parent.session_count, 3);

        let backfill = storage
            .run_runtime_rollup_backfill()
            .expect("run production runtime backfill");
        assert_eq!(backfill.terminal, RollupBackfillTerminal::Completed);
        for (scope, reference) in [(None, all), (Some("parent_only"), parent)] {
            let production =
                with_pinned_query_now(pinned_now, || storage.get_llm_runtime_stats("24h", scope))
                    .expect("read production hybrid runtime");
            require_runtime_parity(&reference, &production, "24h", scope.unwrap_or("all"))
                .expect("independent reference must match production hybrid");
        }
    }
}
