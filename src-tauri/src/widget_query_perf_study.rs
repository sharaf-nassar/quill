//! Read-only benchmark protocol for feature 020 widget queries.
//!
//! The spike binary opens a fresh [`Storage`] reader for every measurement,
//! bypassing all app-level caches while preserving the production query and
//! post-processing paths. A thread-local clock pins every range to one exact
//! endpoint. The frozen corpus is never migrated, cleaned up, or written.

use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;

use crate::integrations::IntegrationProvider;
use crate::models::{ModelRange, UsageBucket};
use crate::storage::{Storage, with_pinned_query_now};

/// One cold, app-cache-bypassed endpoint measurement.
#[derive(Debug, Serialize)]
pub struct WidgetQueryMeasurement {
    pub query: &'static str,
    pub window: &'static str,
    pub start: String,
    pub end: String,
    pub elapsed_ms: f64,
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
    operation: impl FnOnce(&Storage) -> Result<T, String>,
) -> Result<WidgetQueryMeasurement, String> {
    let storage = Storage::init_widget_query_benchmark(corpus)?;
    let (elapsed_ms, output_bytes) = with_pinned_query_now(pinned_end, || {
        let started = Instant::now();
        let value = operation(&storage)?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let output_bytes = serde_json::to_vec(&value)
            .map_err(|error| format!("Serialize {query} benchmark output: {error}"))?
            .len();
        Ok::<_, String>((elapsed_ms, output_bytes))
    })?;

    Ok(WidgetQueryMeasurement {
        query,
        window: window.label,
        start: (pinned_end - window.duration).to_rfc3339(),
        end: pinned_end.to_rfc3339(),
        elapsed_ms,
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
    })
}
