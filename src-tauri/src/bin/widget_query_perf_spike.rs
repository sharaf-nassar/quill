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
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use chrono::{DateTime, Utc};
use quill_lib::widget_query_perf_study::{
    WidgetQueryBenchmarkReport, audit_flagged_paths_ab, audit_model_history_24h_ab,
    audit_pending_flagged_paths_ab, audit_widget_query_plans, backfill_model_rollup_copy,
    backfill_runtime_rollup_copy, clear_query_planner_stats_for_audit, hash_model_rollup_queries,
    measure_model_rollup_queries, measure_runtime_90d, measure_session_breakdown_30d,
    profile_model_overview_queries, run_widget_query_baseline, verify_model_rollup_copy,
    verify_model_rollup_from_frozen, verify_runtime_parity_from_frozen,
};
use rusqlite::{Connection, OpenFlags, ToSql, backup::Backup, backup::StepResult, params};
use serde::Serialize;

fn usage() -> &'static str {
    "Usage:\n  widget_query_perf_spike freeze SOURCE DEST\n  widget_query_perf_spike prepare-analyze-audit COPY\n  widget_query_perf_spike secure-analyze-audit COPY\n  widget_query_perf_spike audit-analyze COPY PINNED_END_RFC3339 [REPORT]\n  widget_query_perf_spike audit-model-history-ab COPY PINNED_END_RFC3339 [SAMPLES] [REPORT]\n  widget_query_perf_spike audit-flagged-paths-ab COPY PINNED_END_RFC3339 [SAMPLES] [REPORT]\n  widget_query_perf_spike audit-pending-paths-ab COPY PINNED_END_RFC3339 [SAMPLES] [REPORT]\n  widget_query_perf_spike backfill-model COPY\n  widget_query_perf_spike verify-model-rollup COPY PINNED_END_RFC3339\n  widget_query_perf_spike verify-model-rollup-derived SOURCE FIXTURE PINNED_END_RFC3339\n  widget_query_perf_spike verify-runtime-parity-derived SOURCE FIXTURE PINNED_END_RFC3339\n  widget_query_perf_spike backfill-runtime COPY\n  widget_query_perf_spike diagnose-model CORPUS PINNED_END_RFC3339\n  widget_query_perf_spike diagnose-project CORPUS PINNED_END_RFC3339 DAYS\n  widget_query_perf_spike hash-model CORPUS PINNED_END_RFC3339\n  widget_query_perf_spike measure-model CORPUS PINNED_END_RFC3339\n  widget_query_perf_spike profile-model CORPUS PINNED_END_RFC3339\n  widget_query_perf_spike measure-runtime CORPUS PINNED_END_RFC3339\n  widget_query_perf_spike measure-session-breakdown CORPUS PINNED_END_RFC3339 [SAMPLES]\n  widget_query_perf_spike measure CORPUS PINNED_END_RFC3339"
}

fn path_arg(value: Option<OsString>, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| format!("Missing {name}.\n{}", usage()).into())
}

fn emit_json_report<T: Serialize>(
    report: &T,
    report_path: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let Some(report_path) = report_path else {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    };
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report_path)?;
    let mut writer = BufWriter::new(&file);
    serde_json::to_writer_pretty(&mut writer, report)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    drop(writer);
    file.sync_all()?;
    println!("report={}", report_path.display());
    Ok(())
}

fn secure_analyze_audit_files(corpus: &Path) -> Result<(), Box<dyn Error>> {
    let mut paths = vec![corpus.to_path_buf()];
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = corpus.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&sidecar)?
            .sync_all()?;
        paths.push(sidecar);
    }
    for path in paths {
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        #[cfg(not(unix))]
        {
            let mut permissions = fs::metadata(&path)?.permissions();
            permissions.set_readonly(false);
            fs::set_permissions(&path, permissions)?;
        }
    }
    Ok(())
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
    println!("| Query | Window | Start | End | Cold | Warm | Output bytes | SHA-256 |");
    println!("| --- | --- | --- | --- | ---: | ---: | ---: | --- |");
    for measurement in &report.measurements {
        println!(
            "| `{}` | {} | `{}` | `{}` | {:.3} ms | {:.3} ms | {} | `{}` |",
            measurement.query,
            measurement.window,
            measurement.start,
            measurement.end,
            measurement.elapsed_ms,
            measurement.warm_elapsed_ms,
            measurement.output_bytes,
            measurement.output_sha256
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

fn completed_project_candidates_sql() -> String {
    let active_predicate =
        "source.processing_status != 'suppressed' AND source.suppressed_sha256 IS NULL";
    format!(
        "WITH scoped_sessions AS (
             SELECT provider, analytics_session_id
             FROM scoped_overview
             GROUP BY provider COLLATE BINARY,
                      analytics_session_id COLLATE BINARY
         ), scoped_chains AS (
             SELECT source.provider,
                    source.analytics_session_id,
                    source.chain_id
             FROM model_observation_sources AS source
                  INDEXED BY idx_model_sources_session
             JOIN scoped_sessions AS scoped
               ON scoped.provider = source.provider
              AND scoped.analytics_session_id = source.analytics_session_id
             WHERE {active_predicate}
               AND source.chain_id IS NOT NULL
             GROUP BY source.provider COLLATE BINARY,
                      source.analytics_session_id COLLATE BINARY,
                      source.chain_id COLLATE BINARY
         )
         SELECT scoped.provider, scoped.analytics_session_id, scoped.chain_id,
                (
                    SELECT json_array(
                               COALESCE(
                                   NULLIF(observation.cwd, ''),
                                   NULLIF(owner.cwd, '')
                               ),
                               observation.observed_at_ms,
                               observation.source_ordinal,
                               observation.source_record_key,
                               observation.source_key,
                               observation.id
                           )
                    FROM model_usage_observations AS observation
                         INDEXED BY idx_model_observations_chain_time
                    JOIN model_observation_sources AS owner
                      ON owner.provider = observation.provider
                     AND owner.source_key = observation.source_key
                    WHERE owner.processing_status != 'suppressed'
                      AND owner.suppressed_sha256 IS NULL
                      AND observation.provider = scoped.provider
                      AND observation.analytics_session_id =
                          scoped.analytics_session_id
                      AND observation.chain_id = scoped.chain_id
                      AND observation.observed_at_ms >= ?1
                      AND observation.observed_at_ms < ?2
                      AND COALESCE(
                              NULLIF(observation.cwd, ''),
                              NULLIF(owner.cwd, '')
                          ) IS NOT NULL
                    ORDER BY observation.observed_at_ms DESC,
                             observation.source_ordinal DESC,
                             observation.source_record_key COLLATE BINARY DESC,
                             observation.source_key COLLATE BINARY DESC,
                             observation.id DESC
                    LIMIT 1
                ) AS packed_candidate
         FROM scoped_chains AS scoped"
    )
}

fn diagnose_completed_project_stage(
    connection: &Connection,
    range_label: &str,
    range_start_ms: i64,
    range_end_ms: i64,
) -> Result<(), Box<dyn Error>> {
    let hour_ms = 3_600_000_i64;
    let rollup_start_ms = range_start_ms
        .div_euclid(hour_ms)
        .saturating_add(i64::from(range_start_ms.rem_euclid(hour_ms) != 0))
        .saturating_mul(hour_ms);
    let rollup_end_ms = range_end_ms.div_euclid(hour_ms).saturating_mul(hour_ms);
    let active_predicate =
        "source.processing_status != 'suppressed' AND source.suppressed_sha256 IS NULL";
    connection.execute_batch("DROP TABLE IF EXISTS scoped_overview;")?;
    connection.execute(
        &format!(
            "CREATE TEMP TABLE scoped_overview AS
             SELECT rollup.provider, rollup.analytics_session_id
             FROM model_usage_hourly AS rollup
             JOIN model_observation_sources AS source
               ON source.provider = rollup.provider
              AND source.source_key = rollup.source_key
             WHERE {active_predicate}
               AND rollup.hour_utc >= ?3
               AND rollup.hour_utc < ?4
             UNION ALL
             SELECT observation.provider, observation.analytics_session_id
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
        params![range_start_ms, range_end_ms, rollup_start_ms, rollup_end_ms],
    )?;
    connection.execute_batch(
        "CREATE INDEX temp.scoped_overview_provider_session
             ON scoped_overview(provider, analytics_session_id);",
    )?;

    let project_sql = completed_project_candidates_sql();
    print_query_plan(
        connection,
        &format!("overview.completed_project_stage_{range_label}"),
        &project_sql,
        &[&range_start_ms, &range_end_ms],
    )?;
    let started = std::time::Instant::now();
    let mut statement = connection.prepare(&project_sql)?;
    let rows = statement.query_map(params![range_start_ms, range_end_ms], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut chain_rows = 0_i64;
    let mut candidate_rows = 0_i64;
    let mut packed_bytes = 0_usize;
    for row in rows {
        let (_, _, _, candidate) = row?;
        chain_rows += 1;
        if let Some(candidate) = candidate {
            candidate_rows += 1;
            packed_bytes += candidate.len();
        }
    }
    println!("overview.completed_project_stage_{range_label}.chains={chain_rows}");
    println!("overview.completed_project_stage_{range_label}.candidates={candidate_rows}");
    println!("overview.completed_project_stage_{range_label}.packed_bytes={packed_bytes}");
    println!(
        "overview.completed_project_stage_{range_label}.elapsed_ms={:.3}",
        started.elapsed().as_secs_f64() * 1_000.0
    );
    Ok(())
}

fn diagnose_completed_project_stage_cold(
    corpus: &Path,
    pinned_end: DateTime<Utc>,
    days: i64,
) -> Result<(), Box<dyn Error>> {
    if !matches!(days, 30 | 90) {
        return Err("Project diagnostic DAYS must be 30 or 90".into());
    }
    let canonical = corpus.canonicalize()?;
    let metadata = canonical.metadata()?;
    if !metadata.permissions().readonly() {
        return Err(format!(
            "Project diagnostic corpus must be read-only: {}",
            canonical.display()
        )
        .into());
    }
    let connection = Connection::open_with_flags(
        &canonical,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.execute_batch("PRAGMA temp_store = MEMORY;")?;
    let range_end_ms = pinned_end.timestamp_millis();
    let range_start_ms = range_end_ms - chrono::TimeDelta::days(days).num_milliseconds();
    diagnose_completed_project_stage(
        &connection,
        &format!("{days}d"),
        range_start_ms,
        range_end_ms,
    )
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
    connection.execute_batch("PRAGMA temp_store = MEMORY;")?;

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

    let source_cwd_counts: (i64, i64, i64, i64) = connection.query_row(
        &format!(
            "SELECT COUNT(*),
                    SUM(NULLIF(source.cwd, '') IS NOT NULL),
                    COUNT(DISTINCT source.provider || char(31)
                                           || source.analytics_session_id),
                    COUNT(DISTINCT CASE WHEN NULLIF(source.cwd, '') IS NOT NULL
                                        THEN source.provider || char(31)
                                             || source.analytics_session_id END)
             FROM model_observation_sources AS source
             WHERE {active_predicate}"
        ),
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    println!("project.active_sources={}", source_cwd_counts.0);
    println!("project.sources_with_cwd={}", source_cwd_counts.1);
    println!("project.active_sessions={}", source_cwd_counts.2);
    println!("project.sessions_with_source_cwd={}", source_cwd_counts.3);

    let observation_cwd_counts: (i64, i64) = connection.query_row(
        &format!(
            "SELECT COUNT(*),
                    COUNT(DISTINCT observation.provider || char(31)
                                            || observation.analytics_session_id)
             FROM model_usage_observations AS observation
             JOIN model_observation_sources AS source
               ON source.provider = observation.provider
              AND source.source_key = observation.source_key
             WHERE {active_predicate}
               AND observation.observed_at_ms >= ?1
               AND observation.observed_at_ms < ?2
               AND NULLIF(observation.cwd, '') IS NOT NULL"
        ),
        params![range_start_30d_ms, range_end_ms],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    println!(
        "project.observations_with_cwd_30d={}",
        observation_cwd_counts.0
    );
    println!(
        "project.sessions_with_observation_cwd_30d={}",
        observation_cwd_counts.1
    );

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
    diagnose_completed_project_stage(&connection, "30d", range_start_30d_ms, range_end_ms)?;
    diagnose_completed_project_stage(&connection, "90d", range_start_90d_ms, range_end_ms)?;
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
        "prepare-analyze-audit" => {
            let corpus = path_arg(args.next(), "COPY")?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            clear_query_planner_stats_for_audit(&corpus)?;
            secure_analyze_audit_files(&corpus)?;
            println!("prepared={}", corpus.display());
            Ok(())
        }
        "secure-analyze-audit" => {
            let corpus = path_arg(args.next(), "COPY")?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            secure_analyze_audit_files(&corpus)?;
            println!("secured={}", corpus.display());
            Ok(())
        }
        "audit-analyze" => {
            let corpus = path_arg(args.next(), "COPY")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            let report_path = args.next().map(PathBuf::from);
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let report = audit_widget_query_plans(&corpus, pinned_end)?;
            emit_json_report(&report, report_path.as_deref())?;
            if report.verdict != "pass" {
                return Err(format!(
                    "Bounded ANALYZE audit failed: plan_regressions={}, timing_regressions={}",
                    report.plan_regressions, report.timing_regressions
                )
                .into());
            }
            Ok(())
        }
        "audit-model-history-ab" => {
            let corpus = path_arg(args.next(), "COPY")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            let samples = args
                .next()
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| "SAMPLES must be valid UTF-8".to_string())?
                        .parse::<usize>()
                        .map_err(|error| format!("Invalid SAMPLES: {error}"))
                })
                .transpose()?
                .unwrap_or(8);
            let report_path = args.next().map(PathBuf::from);
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let report = audit_model_history_24h_ab(&corpus, pinned_end, samples)?;
            emit_json_report(&report, report_path.as_deref())?;
            if report.verdict != "pass" {
                return Err(format!(
                    "Focused model-history A/B failed: median_delta_ms={:.3}, median_ratio={:.3}",
                    report.median_delta_ms, report.median_ratio
                )
                .into());
            }
            Ok(())
        }
        "audit-flagged-paths-ab" => {
            let corpus = path_arg(args.next(), "COPY")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            let samples = args
                .next()
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| "SAMPLES must be valid UTF-8".to_string())?
                        .parse::<usize>()
                        .map_err(|error| format!("Invalid SAMPLES: {error}"))
                })
                .transpose()?
                .unwrap_or(8);
            let report_path = args.next().map(PathBuf::from);
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let report = audit_flagged_paths_ab(&corpus, pinned_end, samples)?;
            emit_json_report(&report, report_path.as_deref())?;
            if report.verdict != "pass" {
                return Err("Focused flagged-path A/B failed".into());
            }
            Ok(())
        }
        "audit-pending-paths-ab" => {
            let corpus = path_arg(args.next(), "COPY")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            let samples = args
                .next()
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| "SAMPLES must be valid UTF-8".to_string())?
                        .parse::<usize>()
                        .map_err(|error| format!("Invalid SAMPLES: {error}"))
                })
                .transpose()?
                .unwrap_or(8);
            let report_path = args.next().map(PathBuf::from);
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let report = audit_pending_flagged_paths_ab(&corpus, pinned_end, samples)?;
            emit_json_report(&report, report_path.as_deref())?;
            if report.verdict != "pass" {
                return Err("Focused pending-path A/B failed".into());
            }
            Ok(())
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
        "verify-runtime-parity-derived" => {
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
            let report = verify_runtime_parity_from_frozen(&source, &fixture, pinned_end)?;
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
        "diagnose-project" => {
            let corpus = path_arg(args.next(), "CORPUS")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            let days = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing DAYS.\n{}", usage()))?
                .parse::<i64>()?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            diagnose_completed_project_stage_cold(&corpus, pinned_end, days)
        }
        "hash-model" => {
            let corpus = path_arg(args.next(), "CORPUS")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            for output in hash_model_rollup_queries(&corpus, pinned_end)? {
                println!(
                    "query={} window={} output_bytes={} output_sha256={}",
                    output.query, output.window, output.output_bytes, output.output_sha256
                );
            }
            Ok(())
        }
        "measure-model" => {
            let corpus = path_arg(args.next(), "CORPUS")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            for measurement in measure_model_rollup_queries(&corpus, pinned_end)? {
                println!(
                    "query={} window={} cold_ms={:.3} warm_ms={:.3} output_bytes={} output_sha256={}",
                    measurement.query,
                    measurement.window,
                    measurement.elapsed_ms,
                    measurement.warm_elapsed_ms,
                    measurement.output_bytes,
                    measurement.output_sha256
                );
            }
            Ok(())
        }
        "profile-model" => {
            let corpus = path_arg(args.next(), "CORPUS")?;
            let pinned_end = args
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or_else(|| format!("Missing PINNED_END_RFC3339.\n{}", usage()))?;
            if args.next().is_some() {
                return Err(format!("Unexpected argument.\n{}", usage()).into());
            }
            let pinned_end = DateTime::parse_from_rfc3339(&pinned_end)?.with_timezone(&Utc);
            let profiles = profile_model_overview_queries(&corpus, pinned_end)?;
            println!("{}", serde_json::to_string_pretty(&profiles)?);
            Ok(())
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
            println!("runtime_90d.output_sha256={}", measurement.output_sha256);
            Ok(())
        }
        _ => Err(format!("Unknown mode {mode:?}.\n{}", usage()).into()),
    }
}
