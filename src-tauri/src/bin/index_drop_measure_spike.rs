//! One-off footprint and cost measurement for the retention index drop.
//!
//! The `EXPLAIN QUERY PLAN` gate (`eqp_index_drop_spike`) answers *may we drop
//! `idx_session_events_provider_source`*. This spike answers *what does the
//! drop buy and what does it cost*, on a production-sized copy:
//!
//! * whole-file bytes before and after the `compact_database` VACUUM, since
//!   `DROP INDEX` frees SQLite pages but no filesystem bytes on its own;
//! * `DROP INDEX` wall time — the drop runs inside `ensure_startup_indexes`,
//!   on the first-open path to a usable app, so this is a one-time cost every
//!   user pays and it has to be known before it ships;
//! * the WAL bytes the drop itself produces, because the drop precedes any
//!   compaction preflight and has no disk budget of its own.
//!
//! These are observations, not thresholds: they are corpus-dependent and this
//! binary asserts nothing about them. It only fails if the source database
//! does not carry the index at all, which would make the run meaningless.
//!
//! Run with `QUILL_INDEX_DROP_DB=/path/to/usage.db cargo run --release --bin
//! index_drop_measure_spike`. The source is opened **read-only** and copied
//! with `VACUUM INTO`, which is safe against the live app writing
//! concurrently. The copy is therefore already compacted, which makes the
//! before/after delta the index's own footprint rather than that footprint
//! plus whatever unrelated free pages the source happened to be carrying —
//! the stricter, more honest number.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use rusqlite::{Connection, OpenFlags, params};

const DROPPED_INDEX: &str = "idx_session_events_provider_source";

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn wal_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push("-wal");
    PathBuf::from(name)
}

fn index_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
        params![name],
        |row| row.get(0),
    )?;
    Ok(count == 1)
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = PathBuf::from(std::env::var("QUILL_INDEX_DROP_DB").map_err(
        |_| "set QUILL_INDEX_DROP_DB to a production-sized usage.db to measure against",
    )?);

    let reader = Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    if !index_exists(&reader, DROPPED_INDEX)? {
        return Err(format!(
            "{} carries no {DROPPED_INDEX}; nothing to measure",
            source.display()
        )
        .into());
    }

    let temp_dir = tempfile::tempdir()?;
    let copy_path = temp_dir.path().join("usage-copy.db");

    // `VACUUM INTO` takes a read snapshot, so the live app may keep writing.
    let copy_started = Instant::now();
    reader.execute("VACUUM INTO ?1", params![copy_path.to_string_lossy()])?;
    let copy_wall = copy_started.elapsed();
    drop(reader);

    let working = Connection::open(&copy_path)?;
    working.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    let bytes_before = file_size(&copy_path);
    let wal_before = file_size(&wal_path(&copy_path));

    let drop_started = Instant::now();
    working.execute_batch(&format!("DROP INDEX IF EXISTS {DROPPED_INDEX};"))?;
    let drop_wall = drop_started.elapsed();
    let wal_after_drop = file_size(&wal_path(&copy_path));

    if index_exists(&working, DROPPED_INDEX)? {
        return Err(format!("{DROPPED_INDEX} survived its own DROP").into());
    }
    working.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let bytes_after_drop = file_size(&copy_path);
    drop(working);

    // A dedicated connection, mirroring `Storage::vacuum_database`.
    let maintenance = Connection::open(&copy_path)?;
    let vacuum_started = Instant::now();
    maintenance.execute_batch("PRAGMA busy_timeout = 5000; VACUUM;")?;
    let vacuum_wall = vacuum_started.elapsed();
    drop(maintenance);
    let bytes_after = file_size(&copy_path);

    println!("source_path={}", source.display());
    println!("source_bytes={}", file_size(&source));
    println!("copy_vacuum_into_wall_time_ms={}", copy_wall.as_millis());
    println!("bytes_before={bytes_before}");
    println!("wal_bytes_before={wal_before}");
    println!("drop_index_wall_time_ms={}", drop_wall.as_millis());
    println!("wal_bytes_after_drop={wal_after_drop}");
    println!(
        "drop_wal_delta_bytes={}",
        wal_after_drop.saturating_sub(wal_before)
    );
    println!("bytes_after_drop_before_vacuum={bytes_after_drop}");
    println!("vacuum_wall_time_ms={}", vacuum_wall.as_millis());
    println!("bytes_after={bytes_after}");
    println!(
        "reclaimed_bytes={}",
        bytes_before.saturating_sub(bytes_after)
    );
    Ok(())
}
