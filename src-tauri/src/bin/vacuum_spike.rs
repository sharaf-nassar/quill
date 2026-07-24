//! One-off, reproducible VACUUM sizing spike for analytics-query-perf.
//!
//! This is deliberately separate from the application maintenance path.  It
//! establishes the wall-time and ingest-boundary assumptions that the later
//! compact-database implementation must satisfy.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use rusqlite::{Connection, params};

const TARGET_BYTES: u64 = 7_450_000_000;
const LEGACY_BYTES: u64 = 870_000_000;
const CHUNK_BYTES: u64 = 64 * 1024 * 1024;

/// Small prototype of the process-wide guard the HTTP/backfill boundary will
/// consult.  A caller seeing `true` must retry rather than write or drop data.
struct QuiesceFlag(AtomicBool);

impl QuiesceFlag {
    fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn quiesce(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn resume(&self) {
        self.0.store(false, Ordering::Release);
    }

    fn ingest_must_retry(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

fn insert_zeros(conn: &mut Connection, table: &str, mut bytes: u64) -> rusqlite::Result<()> {
    let transaction = conn.transaction()?;
    let statement = format!("INSERT INTO {table} (payload) VALUES (zeroblob(?1))");
    while bytes > 0 {
        let chunk = bytes.min(CHUNK_BYTES) as i64;
        transaction.execute(&statement, params![chunk])?;
        bytes -= chunk as u64;
    }
    transaction.commit()
}

fn file_size(path: &Path) -> std::io::Result<u64> {
    Ok(fs::metadata(path)?.len())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = tempfile::tempdir()?;
    let db_path: PathBuf = temp_dir.path().join("usage-copy.sqlite3");
    let retained_bytes = TARGET_BYTES - LEGACY_BYTES;

    let mut writer = Connection::open(&db_path)?;
    writer.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE retained_data (payload BLOB NOT NULL);
         CREATE TABLE tool_actions_legacy_v30 (payload BLOB NOT NULL);",
    )?;
    insert_zeros(&mut writer, "retained_data", retained_bytes)?;
    insert_zeros(&mut writer, "tool_actions_legacy_v30", LEGACY_BYTES)?;
    writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;

    let bytes_before = file_size(&db_path)?;
    writer.execute_batch("DROP TABLE tool_actions_legacy_v30; PRAGMA wal_checkpoint(TRUNCATE);")?;
    drop(writer);

    let quiesce = QuiesceFlag::new();
    quiesce.quiesce();
    assert!(quiesce.ingest_must_retry(), "quiesced ingest must retry");

    // This intentionally opens a distinct maintenance connection.  The later
    // app command must preserve that separation from Storage::conn.
    let started = Instant::now();
    let maintenance = Connection::open(&db_path)?;
    maintenance.execute_batch("PRAGMA busy_timeout = 5000; VACUUM;")?;
    let wall_time = started.elapsed();

    quiesce.resume();
    assert!(
        !quiesce.ingest_must_retry(),
        "ingest must resume after maintenance"
    );

    let bytes_after = file_size(&db_path)?;
    println!("target_bytes={TARGET_BYTES}");
    println!("bytes_before={bytes_before}");
    println!("bytes_after={bytes_after}");
    println!(
        "reclaimed_bytes={}",
        bytes_before.saturating_sub(bytes_after)
    );
    println!("vacuum_wall_time_ms={}", wall_time.as_millis());
    println!("quiesce_prototype=retry_while_active,resume_after_maintenance");
    Ok(())
}
