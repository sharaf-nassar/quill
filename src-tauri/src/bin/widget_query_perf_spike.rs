//! Frozen-corpus benchmark harness for feature 020 widget queries.
//!
//! Two explicit modes keep corpus creation separate from measurement:
//!
//! ```text
//! cargo run --release --bin widget_query_perf_spike -- freeze SOURCE DEST
//! cargo run --release --bin widget_query_perf_spike -- measure CORPUS PINNED_END
//! ```
//!
//! `freeze` uses SQLite's online backup API from a read-only source
//! connection, refuses to overwrite a destination, verifies the copied
//! database, then removes write permission. `measure` refuses a writable
//! corpus and every storage connection is opened read-only.

use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use quill_lib::widget_query_perf_study::{
    WidgetQueryBenchmarkReport, backfill_model_rollup_copy, backfill_runtime_rollup_copy,
    measure_runtime_90d, measure_session_breakdown_30d, run_widget_query_baseline,
    verify_model_rollup_copy, verify_model_rollup_from_frozen,
};
use rusqlite::{Connection, OpenFlags, ToSql, backup::Backup, backup::StepResult, params};

fn usage() -> &'static str {
    "Usage:\n  widget_query_perf_spike freeze SOURCE DEST\n  widget_query_perf_spike backfill-model COPY\n  widget_query_perf_spike verify-model-rollup COPY PINNED_END_RFC3339\n  widget_query_perf_spike verify-model-rollup-derived SOURCE FIXTURE PINNED_END_RFC3339\n  widget_query_perf_spike backfill-runtime COPY\n  widget_query_perf_spike diagnose-model CORPUS PINNED_END_RFC3339\n  widget_query_perf_spike measure-runtime CORPUS PINNED_END_RFC3339\n  widget_query_perf_spike measure-session-breakdown CORPUS PINNED_END_RFC3339 [SAMPLES]\n  widget_query_perf_spike measure CORPUS PINNED_END_RFC3339"
}

fn path_arg(value: Option<OsString>, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| format!("Missing {name}.\n{}", usage()).into())
}

fn freeze_snapshot(source: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    let source = source.canonicalize()?;
    if !source.is_file() {
        return Err(format!("Snapshot source is not a file: {}", source.display()).into());
    }
    if destination.exists() {
        return Err(format!(
            "Refusing to overwrite snapshot destination: {}",
            destination.display()
        )
        .into());
    }
    let parent = destination
        .parent()
        .ok_or("Snapshot destination must have a parent directory")?;
    fs::create_dir_all(parent)?;

    let source_connection = Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    source_connection.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 5000;")?;
    let mut destination_connection = Connection::open_with_flags(
        destination,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    {
        let backup = Backup::new(&source_connection, &mut destination_connection)?;
        loop {
            match backup.step(-1)? {
                StepResult::Done => break,
                StepResult::More | StepResult::Busy | StepResult::Locked => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                unexpected => {
                    return Err(format!("Unexpected SQLite backup state: {unexpected:?}").into());
                }
            }
        }
    }

    let integrity: String =
        destination_connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(format!("Frozen snapshot quick_check failed: {integrity}").into());
    }
    let page_count: i64 =
        destination_connection.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    drop(destination_connection);
    drop(source_connection);

    let mut permissions = fs::metadata(destination)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(destination, permissions)?;
    let metadata = fs::metadata(destination)?;
    println!("source={}", source.display());
    println!("destination={}", destination.display());
    println!("bytes={}", metadata.len());
    println!("pages={page_count}");
    println!("quick_check=ok");
    println!("read_only={}", metadata.permissions().readonly());
    Ok(())
}

fn print_markdown(report: &WidgetQueryBenchmarkReport) {
    println!("# Widget query measurement");
    println!();
    println!("- Corpus: `{}`", report.corpus_path);
    println!("- Corpus bytes: `{}`", report.corpus_bytes);
    println!("- Corpus read-only: `{}`", report.corpus_read_only);
    println!("- SQLite: `{}`", report.sqlite_version);
    println!("- Schema version: `{}`", report.schema_version);
    println!(
        "- Pages: `{}` × `{}` bytes; freelist `{}`",
        report.page_count, report.page_size, report.freelist_count
    );
    println!("- Pinned end: `{}`", report.pinned_end);
    println!("- OS page cache: {}", report.os_page_cache);
    println!();
    println!("| Query | Window | Start | End | Cold | Warm | Output bytes |");
    println!("| --- | --- | --- | --- | ---: | ---: | ---: |");
    for measurement in &report.measurements {
        println!(
            "| `{}` | {} | `{}` | `{}` | {:.3} ms | {:.3} ms | {} |",
            measurement.query,
            measurement.window,
            measurement.start,
            measurement.end,
            measurement.elapsed_ms,
            measurement.warm_elapsed_ms,
            measurement.output_bytes
        );
    }
    println!();
    println!("## 30d view backend fan-outs");
    println!();
    println!(
        "These totals execute each view's calls in the listed order on one shared storage handle. They exclude IPC, React, layout, paint, and all other frontend/render work."
    );
    println!();
    println!("| View | Window | Cold backend total | Warm backend total | Output bytes | Calls |");
    println!("| --- | --- | ---: | ---: | ---: | --- |");
    for measurement in &report.view_fanouts {
        println!(
            "| {} | {} | {:.3} ms | {:.3} ms | {} | {} |",
            measurement.view,
            measurement.window,
            measurement.cold_elapsed_ms,
            measurement.warm_elapsed_ms,
            measurement.output_bytes,
            measurement.calls
        );
    }
}

fn print_query_plan(
    connection: &Connection,
    label: &str,
    sql: &str,
    parameters: &[&dyn ToSql],
) -> Result<(), Box<dyn Error>> {
    println!("{label}.eqp:");
    let mut statement = connection.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
    let rows = statement.query_map(parameters, |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, parent, detail) = row?;
        println!("  {id}:{parent} {detail}");
    }
    Ok(())
}

fn diagnose_model_residuals(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
) -> Result<(), Box<dyn Error>> {
    let canonical = corpus.canonicalize()?;
    let metadata = canonical.metadata()?;
    if !metadata.permissions().readonly() {
        return Err(format!(
            "Model diagnostic corpus must be read-only: {}",
            canonical.display()
        )
        .into());
    }
    let connection = Connection::open_with_flags(
        &canonical,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch("PRAGMA query_only = ON;")?;

    let range_end_ms = pinned_end.timestamp_millis();
    let range_start_30d_ms = range_end_ms - chrono::TimeDelta::days(30).num_milliseconds();
    let range_start_90d_ms = range_end_ms - chrono::TimeDelta::days(90).num_milliseconds();
    let hour_ms = 3_600_000_i64;
    let day_ms = chrono::TimeDelta::days(1).num_milliseconds();
    let rollup_start_ms = range_start_30d_ms
        .div_euclid(hour_ms)
        .saturating_add(i64::from(range_start_30d_ms.rem_euclid(hour_ms) != 0))
        .saturating_mul(hour_ms);
    let rollup_end_ms = range_end_ms.div_euclid(hour_ms).saturating_mul(hour_ms);

    let rollup_state: (String, String, i64) = connection.query_row(
        "SELECT model_backfill_status, runtime_backfill_status, rollup_generation
         FROM rollup_meta WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    println!("model_diagnostic.corpus={}", canonical.display());
    println!("model_diagnostic.pinned_end={}", pinned_end.to_rfc3339());
    println!("rollup.model_status={}", rollup_state.0);
    println!("rollup.runtime_status={}", rollup_state.1);
    println!("rollup.generation={}", rollup_state.2);

    for (label, table) in [
        ("model", "model_usage_hourly"),
        ("runtime", "runtime_hourly"),
    ] {
        let (rows, raw_pruned): (i64, i64) = connection.query_row(
            &format!("SELECT COUNT(*), COALESCE(SUM(raw_pruned = 1), 0) FROM {table}"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        println!("rollup.{label}_rows={rows}");
        println!("rollup.{label}_raw_pruned_rows={raw_pruned}");
    }

    let active_predicate =
        "source.processing_status != 'suppressed' AND source.suppressed_sha256 IS NULL";
    let active_rows_sql = format!(
        "SELECT COUNT(*)
         FROM model_usage_observations AS observation
         JOIN model_observation_sources AS source
           ON source.provider = observation.provider
          AND source.source_key = observation.source_key
         WHERE {active_predicate}
           AND observation.observed_at_ms >= ?1
           AND observation.observed_at_ms < ?2"
    );
    for (label, range_start_ms) in [("30d", range_start_30d_ms), ("90d", range_start_90d_ms)] {
        let rows: i64 = connection.query_row(
            &active_rows_sql,
            params![range_start_ms, range_end_ms],
            |row| row.get(0),
        )?;
        println!("raw.active_rows_{label}={rows}");
    }

    let rollup_rows_30d: i64 = connection.query_row(
        "SELECT COUNT(*) FROM model_usage_hourly
         WHERE hour_utc >= ?1 AND hour_utc < ?2",
        params![rollup_start_ms, rollup_end_ms],
        |row| row.get(0),
    )?;
    let raw_open_boundary_rows_30d: i64 = connection.query_row(
        &format!(
            "SELECT COUNT(*)
             FROM model_usage_observations AS observation
             JOIN model_observation_sources AS source
               ON source.provider = observation.provider
              AND source.source_key = observation.source_key
             WHERE {active_predicate}
               AND observation.observed_at_ms >= ?1
               AND observation.observed_at_ms < ?2
               AND (observation.observed_at_ms < ?3
                    OR observation.observed_at_ms >= ?4)"
        ),
        params![
            range_start_30d_ms,
            range_end_ms,
            rollup_start_ms,
            rollup_end_ms
        ],
        |row| row.get(0),
    )?;
    println!("hybrid.rollup_rows_30d={rollup_rows_30d}");
    println!("hybrid.open_boundary_raw_rows_30d={raw_open_boundary_rows_30d}");

    let project_sql = format!(
        "WITH ranked AS (
             SELECT observation.provider,
                    observation.analytics_session_id,
                    COALESCE(NULLIF(observation.cwd, ''), NULLIF(source.cwd, ''))
                        AS effective_cwd,
                    ROW_NUMBER() OVER (
                        PARTITION BY observation.provider COLLATE BINARY,
                                     observation.analytics_session_id COLLATE BINARY
                        ORDER BY CASE
                                     WHEN COALESCE(NULLIF(observation.cwd, ''),
                                                   NULLIF(source.cwd, '')) IS NULL
                                     THEN 1 ELSE 0
                                 END,
                                 observation.observed_at_ms DESC,
                                 observation.source_ordinal DESC,
                                 observation.source_record_key COLLATE BINARY DESC,
                                 observation.source_key COLLATE BINARY DESC,
                                 observation.id DESC
                    ) AS cwd_rank
             FROM model_usage_observations AS observation
             JOIN model_observation_sources AS source
               ON source.provider = observation.provider
              AND source.source_key = observation.source_key
             WHERE {active_predicate}
               AND observation.observed_at_ms >= ?1
               AND observation.observed_at_ms < ?2
         )
         SELECT COUNT(*) FROM ranked WHERE cwd_rank = 1"
    );
    print_query_plan(
        &connection,
        "overview.raw_project_stage",
        &project_sql,
        &[&range_start_30d_ms, &range_end_ms],
    )?;
    let project_started = std::time::Instant::now();
    let project_rows: i64 = connection.query_row(
        &project_sql,
        params![range_start_30d_ms, range_end_ms],
        |row| row.get(0),
    )?;
    println!("overview.raw_project_stage.rows={project_rows}");
    println!(
        "overview.raw_project_stage.elapsed_ms={:.3}",
        project_started.elapsed().as_secs_f64() * 1_000.0
    );

    let history_raw_sql = format!(
        "SELECT COUNT(*)
         FROM model_usage_observations AS observation
         JOIN model_observation_sources AS source
           ON source.provider = observation.provider
          AND source.source_key = observation.source_key
         WHERE {active_predicate}
           AND observation.observed_at_ms >= ?1
           AND observation.observed_at_ms < ?2
           AND NOT (
               observation.observed_at_ms / {hour_ms} * {hour_ms} >= ?4
               AND observation.observed_at_ms / {hour_ms} * {hour_ms} < ?5
               AND (observation.observed_at_ms / {hour_ms} * {hour_ms} - ?1) / ?3
                   = (observation.observed_at_ms / {hour_ms} * {hour_ms}
                      + {hour_ms} - 1 - ?1) / ?3
           )
           AND NOT EXISTS (
               SELECT 1 FROM model_usage_hourly AS authoritative
               WHERE authoritative.hour_utc =
                     observation.observed_at_ms / {hour_ms} * {hour_ms}
                 AND authoritative.provider = observation.provider
                 AND authoritative.source_key = observation.source_key
                 AND authoritative.derived_model_id =
                     COALESCE(observation.derived_model_id, '')
                 AND authoritative.raw_pruned = 1
           )"
    );
    print_query_plan(
        &connection,
        "history.raw_residual_stage",
        &history_raw_sql,
        &[
            &range_start_30d_ms,
            &range_end_ms,
            &day_ms,
            &rollup_start_ms,
            &rollup_end_ms,
        ],
    )?;
    let history_raw_rows: i64 = connection.query_row(
        &history_raw_sql,
        params![
            range_start_30d_ms,
            range_end_ms,
            day_ms,
            rollup_start_ms,
            rollup_end_ms
        ],
        |row| row.get(0),
    )?;
    println!("history.raw_residual_stage.rows={history_raw_rows}");
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| usage().to_string())?;
    match mode.as_str() {
        "freeze" => {
            let source = path_arg(args.next(), "SOURCE")?;
            let destination = path_arg(args.next(), "DEST")?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            freeze_snapshot(&source, &destination)
        }
        "measure" => {
            let corpus = path_arg(args.next(), "CORPUS")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let report = run_widget_query_baseline(&corpus, pinned_end)?;
            print_markdown(&report);
            Ok(())
        }
        "measure-session-breakdown" => {
            let corpus = path_arg(args.next(), "CORPUS")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            let sample_count = args
                .next()
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| "SAMPLES must be valid UTF-8".to_string())?
                        .parse::<usize>()
                        .map_err(|error| format!("Invalid SAMPLES: {error}"))
                })
                .transpose()?
                .unwrap_or(10);
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let report = measure_session_breakdown_30d(&corpus, pinned_end, sample_count)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        "backfill-runtime" => {
            let corpus = path_arg(args.next(), "COPY")?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let started = std::time::Instant::now();
            let (rows_done, rows_total) = backfill_runtime_rollup_copy(&corpus)?;
            println!("runtime_backfill.rows_done={rows_done}");
            println!("runtime_backfill.rows_total={rows_total}");
            println!(
                "runtime_backfill.elapsed_ms={:.3}",
                started.elapsed().as_secs_f64() * 1_000.0
            );
            Ok(())
        }
        "backfill-model" => {
            let corpus = path_arg(args.next(), "COPY")?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let report = backfill_model_rollup_copy(&corpus)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        "verify-model-rollup" => {
            let corpus = path_arg(args.next(), "COPY")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let report = verify_model_rollup_copy(&corpus, pinned_end)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        "verify-model-rollup-derived" => {
            let source = path_arg(args.next(), "SOURCE")?;
            let fixture = path_arg(args.next(), "FIXTURE")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let report = verify_model_rollup_from_frozen(&source, &fixture, pinned_end)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        "diagnose-model" => {
            let corpus = path_arg(args.next(), "CORPUS")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            diagnose_model_residuals(&corpus, pinned_end)
        }
        "measure-runtime" => {
            let corpus = path_arg(args.next(), "CORPUS")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let measurement = measure_runtime_90d(&corpus, pinned_end)?;
            println!("runtime_90d.elapsed_ms={:.3}", measurement.elapsed_ms);
            println!(
                "runtime_90d.warm_elapsed_ms={:.3}",
                measurement.warm_elapsed_ms
            );
            println!("runtime_90d.output_bytes={}", measurement.output_bytes);
            Ok(())
        }
        _ => Err(format!("Unknown mode {mode:?}.\n{}", usage()).into()),
    }
}
