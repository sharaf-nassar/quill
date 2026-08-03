//! Read-only benchmark protocol for feature 020 widget queries.
//!
//! The spike binary opens a fresh [`Storage`] reader for every measurement,
//! bypassing all app-level caches while preserving the production query and
//! post-processing paths. A thread-local clock pins every range to one exact
//! endpoint. The frozen corpus is never migrated, cleaned up, or written.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;

use crate::integrations::IntegrationProvider;
use crate::models::{ModelRange, UsageBucket};
use crate::rollup_backfill::{
    RollupBackfillControls, RollupBackfillProgress, RollupBackfillTerminal, RollupChunkControl,
};
use crate::storage::{Storage, with_pinned_query_now};

/// One cold, app-cache-bypassed endpoint measurement.
#[derive(Debug, Serialize)]
pub struct WidgetQueryMeasurement {
    pub query: &'static str,
    pub window: &'static str,
    pub start: String,
    pub end: String,
    pub elapsed_ms: f64,
    pub warm_elapsed_ms: f64,
    pub output_bytes: usize,
}

/// One complete view's backend request set, measured without frontend work.
#[derive(Debug, Serialize)]
pub struct WidgetViewFanoutMeasurement {
    pub view: &'static str,
    pub window: &'static str,
    pub calls: &'static str,
    pub cold_elapsed_ms: f64,
    pub warm_elapsed_ms: f64,
    pub output_bytes: usize,
}

/// Reproducible metadata and all baseline measurements from one corpus pass.
#[derive(Debug, Serialize)]
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
    let (elapsed_ms, warm_elapsed_ms, output_bytes) = with_pinned_query_now(pinned_end, || {
        let started = Instant::now();
        let value = operation(&storage)?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let output = serde_json::to_vec(&value)
            .map_err(|error| format!("Serialize {query} benchmark output: {error}"))?;

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
        Ok::<_, String>((elapsed_ms, warm_elapsed_ms, output.len()))
    })?;

    Ok(WidgetQueryMeasurement {
        query,
        window: window.label,
        start: (pinned_end - window.duration).to_rfc3339(),
        end: pinned_end.to_rfc3339(),
        elapsed_ms,
        warm_elapsed_ms,
        output_bytes,
    })
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
            let started = Instant::now();
            let cold_output_bytes = operation(&storage)?;
            let cold_elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;

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

/// Run the complete BEFORE query matrix against one immutable corpus.
// @lat: [[backend#Database#Widget query benchmark corpus]]
pub fn run_widget_query_baseline(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
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
    if !metadata.permissions().readonly() {
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
