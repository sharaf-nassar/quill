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
    WidgetQueryBenchmarkReport, backfill_runtime_rollup_copy, measure_runtime_90d,
    run_widget_query_baseline,
};
use rusqlite::{Connection, OpenFlags, backup::Backup, backup::StepResult};

fn usage() -> &'static str {
    "Usage:\n  widget_query_perf_spike freeze SOURCE DEST\n  widget_query_perf_spike backfill-runtime COPY\n  widget_query_perf_spike measure-runtime CORPUS PINNED_END_RFC3339\n  widget_query_perf_spike measure CORPUS PINNED_END_RFC3339"
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
    println!("# Widget query BEFORE baseline");
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
    println!("| Query | Window | Start | End | Cold | Output bytes |");
    println!("| --- | --- | --- | --- | ---: | ---: |");
    for measurement in &report.measurements {
        println!(
            "| `{}` | {} | `{}` | `{}` | {:.3} ms | {} |",
            measurement.query,
            measurement.window,
            measurement.start,
            measurement.end,
            measurement.elapsed_ms,
            measurement.output_bytes
        );
    }
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
            println!("runtime_90d.output_bytes={}", measurement.output_bytes);
            Ok(())
        }
        _ => Err(format!("Unknown mode {mode:?}.\n{}", usage()).into()),
    }
}
