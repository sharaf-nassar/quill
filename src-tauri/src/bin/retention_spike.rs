//! Retention timing spike (feature 014).
//!
//! This binary is the *second* consumer of the frozen synthetic retention
//! corpus. The fixture's whole value is that acceptance tests and budget
//! measurements run on one corpus, and the only way to keep that true is for
//! the spike to link the same `pub` builder the tests use — which this file
//! proves it can, rather than leaving it assumed.
//!
//! It is a **measurement, not a test**: nothing here is a threshold CI
//! enforces. Its output is what fixes the numeric budgets the chunked delete
//! engine and its preflight are built against — chunk size, the per-chunk
//! wall target, the WAL- and TEMP-bytes-per-row constants, the free-space
//! re-check interval `N`, the stale-preview tolerance, the Counting-phase
//! budget and the total wall-time budget — plus one design signal: whether
//! the Counting scan dominates the run badly enough to reopen the
//! two-scan/two-lease split between `preview_retention` and
//! `run_retention_maintenance`.
//!
//! Phases, in order:
//!
//! 0. Build the corpus and check the plan against the database.
//! 1. Counting phase — materialize the two `retention_doomed_*` TEMP tables
//!    under each candidate `PRAGMA temp_store`, measuring wall time, temp
//!    b-tree bytes and resident-memory delta for each.
//! 2. Index sensitivity — the same scans with and without `idx_se_timestamp`,
//!    plus the query plan each one takes. `tool_actions` doubles as the
//!    control: dropping a `session_events` index cannot change its plan, so
//!    whatever ratio it shows is page-cache bias rather than index effect.
//! 3. Delete phase — the full chunked delete at several chunk sizes, on a
//!    fresh copy of the corpus each time, measuring per-chunk transaction
//!    hold, per-chunk WAL bytes and post-`wal_checkpoint(TRUNCATE)` WAL size.
//! 4. Free-space probe cost — what a `statvfs` call costs, which is what
//!    sizes the every-`N`-chunks re-check.
//! 5. Derived budgets and the Counting-phase design signal.
//!
//! Run with `cargo run --release --bin retention_spike`. The scale knobs can
//! be overridden through `QUILL_RETENTION_SPIKE_*` environment variables for
//! a quick smoke run, but the published budgets come from the defaults.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quill_lib::retention_fixture::{
    RetentionFixtureSpec, RetentionRowKind, RetentionTable, build_retention_fixture, count_rows,
};
use rusqlite::{Connection, TransactionBehavior, params};

/// Corpus size: 24 buckets × 16,700 source-owned conforming rows per bucket
/// per table. Retaining 3 buckets leaves ~350k doomed rows in each of the two
/// target tables — the ~700k-row prune the plan sizes against — inside a
/// ~2M-row database.
const SPIKE_MONTHS: u32 = 24;
const SPIKE_OWNED_ROWS_PER_MONTH: u32 = 16_700;
const SPIKE_LIVE_ROWS_PER_MONTH: u32 = 200;
const SPIKE_SOURCES: u32 = 24;

/// Months of history the reported cutoff retains.
const SPIKE_MONTHS_RETAINED: u32 = 3;

/// Chunk sizes swept in the delete phase. Each gets its own copy of the
/// corpus, so they are measured independently rather than one continuing
/// where the last left off.
const CHUNK_SIZES: [u64; 5] = [5_000, 10_000, 25_000, 50_000, 100_000];

/// Repetitions of each scan measurement. The median is reported so a single
/// unlucky I/O stall does not become a published budget.
const SCAN_REPETITIONS: usize = 3;

/// `statvfs` calls timed to price the free-space re-check.
const FREE_SPACE_PROBE_SAMPLES: u32 = 2_000;

/// Longest per-chunk transaction hold that still reads as a live progress
/// bar. The recommended chunk size is the largest swept size whose p95 hold
/// stays under this.
///
/// One second, not the ~250 ms "feels instantaneous" figure: this is a
/// background maintenance job the user watches through a progress bar, and
/// the requirement the plan states is that the bar *visibly advances* rather
/// than that each step feels immediate. One update per second satisfies that,
/// and buying finer granularity than that costs real total wall time, because
/// smaller chunks amortize each chunk's fixed cost over fewer rows.
const RESPONSIVE_CHUNK_HOLD_MS: f64 = 1_000.0;

/// Target wall-clock spacing between free-space re-checks. `N` is derived
/// from this and the measured mean chunk hold.
const FREE_SPACE_RECHECK_TARGET_MS: f64 = 1_000.0;

/// Headroom multiplier applied to every measurement that becomes a budget.
/// Budgets are ceilings a slower machine must still fit under, not the
/// measurement itself.
const BUDGET_HEADROOM: f64 = 3.0;

/// Everything this spike can fail on that is not already an I/O or SQLite
/// error. Kept specific so an unexpected failure keeps its own type instead
/// of being flattened into a string.
#[derive(Debug)]
enum SpikeError {
    /// The corpus does not contain the population the plan predicted.
    CorpusDrift {
        table: &'static str,
        kind: &'static str,
        planned: u64,
        stored: u64,
    },
    /// A chunk loop made no progress but the doomed table was not empty.
    ChunkStalled { table: &'static str, remaining: u64 },
    /// The chunked delete removed a different number of rows than the
    /// Counting phase said were doomed.
    DeleteCountMismatch {
        table: &'static str,
        doomed: u64,
        deleted: u64,
    },
    /// `statvfs` refused to report free space for a path.
    FreeSpace { path: PathBuf, errno: i32 },
    /// No chunk size was swept, so nothing can be recommended.
    NoChunkMeasurements,
}

impl std::fmt::Display for SpikeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpikeError::CorpusDrift {
                table,
                kind,
                planned,
                stored,
            } => write!(
                f,
                "Corpus drift for {table} / {kind}: plan says {planned} rows, database has {stored}"
            ),
            SpikeError::ChunkStalled { table, remaining } => write!(
                f,
                "Chunk loop for {table} stalled with {remaining} doomed rowids remaining"
            ),
            SpikeError::DeleteCountMismatch {
                table,
                doomed,
                deleted,
            } => write!(
                f,
                "Chunked delete removed {deleted} rows from {table} but {doomed} were doomed"
            ),
            SpikeError::FreeSpace { path, errno } => write!(
                f,
                "Read free disk space for {}: errno {errno}",
                path.display()
            ),
            SpikeError::NoChunkMeasurements => {
                write!(f, "No chunk size produced a measurement to recommend from")
            }
        }
    }
}

impl std::error::Error for SpikeError {}

/// One `retention_doomed_*` TEMP table and the target table it belongs to.
#[derive(Clone, Copy)]
struct DoomedTable {
    target: RetentionTable,
    temp_name: &'static str,
}

const DOOMED_TABLES: [DoomedTable; 2] = [
    DoomedTable {
        target: RetentionTable::ToolActions,
        temp_name: "retention_doomed_tool_actions",
    },
    DoomedTable {
        target: RetentionTable::SessionEvents,
        temp_name: "retention_doomed_session_events",
    },
];

/// The exact Counting-phase statement from the plan, for one target table.
fn doomed_scan_sql(doomed: DoomedTable) -> String {
    // Both interpolated fragments are compile-time constants owned by this
    // file and the fixture's own enum; nothing caller-supplied reaches the
    // SQL text.
    format!(
        "CREATE TEMP TABLE {} AS
         SELECT rowid AS rid FROM {}
          WHERE source_key IS NOT NULL
            AND length(timestamp) = 24 AND timestamp LIKE '%Z'
            AND timestamp < ?1",
        doomed.temp_name,
        doomed.target.as_str()
    )
}

/// The scan the Counting phase pays for, without the `CREATE TEMP TABLE`
/// wrapper, so `EXPLAIN QUERY PLAN` describes the scan itself.
fn doomed_select_sql(doomed: DoomedTable) -> String {
    format!(
        "SELECT rowid AS rid FROM {}
          WHERE source_key IS NOT NULL
            AND length(timestamp) = 24 AND timestamp LIKE '%Z'
            AND timestamp < ?1",
        doomed.target.as_str()
    )
}

fn env_u32(name: &str, fallback: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(fallback)
}

/// Resident set size of this process, in bytes.
///
/// Only Linux exposes this cheaply enough to sample around a single
/// statement; elsewhere the MEMORY-vs-FILE `temp_store` comparison rests on
/// the temp b-tree byte count alone.
#[cfg(target_os = "linux")]
fn resident_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: `sysconf` with a valid name has no preconditions and no side
    // effects; a non-positive return is handled below.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return None;
    }
    Some(resident_pages * page_size as u64)
}

#[cfg(not(target_os = "linux"))]
fn resident_bytes() -> Option<u64> {
    None
}

/// Bytes held by files this process has open but that are already unlinked.
///
/// SQLite creates a `temp_store = FILE` spill file and unlinks it
/// immediately, so it never appears in a directory listing. On Linux the
/// still-open descriptor is the only way to see those bytes.
#[cfg(target_os = "linux")]
fn unlinked_open_file_bytes() -> Option<u64> {
    let mut total = 0_u64;
    for entry in std::fs::read_dir("/proc/self/fd").ok()?.flatten() {
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        if !target.to_string_lossy().ends_with(" (deleted)") {
            continue;
        }
        // Stat through the descriptor: the path itself no longer resolves.
        if let Ok(metadata) = std::fs::metadata(entry.path()) {
            total += metadata.len();
        }
    }
    Some(total)
}

#[cfg(not(target_os = "linux"))]
fn unlinked_open_file_bytes() -> Option<u64> {
    None
}

/// Free bytes on the filesystem holding `path`.
fn available_disk_space(path: &Path) -> Result<u64, Box<dyn std::error::Error>> {
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is NUL-terminated and `stats` points to writable
    // storage sized for the struct.
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(Box::new(SpikeError::FreeSpace {
            path: path.to_path_buf(),
            errno: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        }));
    }
    // SAFETY: `statvfs` returned 0, so it initialized the struct.
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

/// Size of the database's write-ahead log right now, or zero if it is absent.
fn wal_bytes(db_path: &Path) -> u64 {
    let mut wal = db_path.as_os_str().to_os_string();
    wal.push("-wal");
    std::fs::metadata(PathBuf::from(wal))
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Bytes the `temp` schema's b-trees currently occupy.
fn temp_schema_bytes(conn: &Connection) -> rusqlite::Result<u64> {
    let pages: i64 = conn.query_row("PRAGMA temp.page_count", [], |row| row.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA temp.page_size", [], |row| row.get(0))?;
    Ok((pages.max(0) as u64).saturating_mul(page_size.max(0) as u64))
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// Nearest-rank percentile over an unsorted sample.
fn percentile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Open a maintenance connection in the shape the delete engine will use: its
/// own connection, WAL, a busy timeout, and an explicitly pinned `temp_store`.
fn open_maintenance(db_path: &Path, temp_store: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch(&format!(
        "PRAGMA busy_timeout = 5000;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = {temp_store};"
    ))?;
    Ok(conn)
}

/// What one Counting-phase pass cost.
struct CountingMeasurement {
    temp_store: &'static str,
    per_table_ms: Vec<(&'static str, f64)>,
    total_ms: f64,
    doomed_rows: u64,
    temp_bytes: u64,
    resident_delta_bytes: i64,
    unlinked_file_bytes: u64,
}

impl CountingMeasurement {
    fn temp_bytes_per_row(&self) -> f64 {
        self.temp_bytes as f64 / self.doomed_rows.max(1) as f64
    }
}

/// Materialize both `retention_doomed_*` TEMP tables and price the pass.
fn measure_counting(
    db_path: &Path,
    cutoff: &str,
    temp_store: &'static str,
) -> Result<CountingMeasurement, Box<dyn std::error::Error>> {
    let conn = open_maintenance(db_path, temp_store)?;
    let resident_before = resident_bytes();

    let mut per_table_ms = Vec::new();
    let mut doomed_rows = 0_u64;
    let mut total_ms = 0.0;
    for doomed in DOOMED_TABLES {
        let started = Instant::now();
        conn.execute(&doomed_scan_sql(doomed), params![cutoff])?;
        let elapsed = millis(started.elapsed());
        per_table_ms.push((doomed.target.as_str(), elapsed));
        total_ms += elapsed;
        let rows: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {}", doomed.temp_name),
            [],
            |row| row.get(0),
        )?;
        doomed_rows += rows.max(0) as u64;
    }

    let temp_bytes = temp_schema_bytes(&conn)?;
    let unlinked_file_bytes = unlinked_open_file_bytes().unwrap_or(0);
    let resident_delta_bytes = match (resident_before, resident_bytes()) {
        (Some(before), Some(after)) => after as i64 - before as i64,
        _ => 0,
    };

    Ok(CountingMeasurement {
        temp_store,
        per_table_ms,
        total_ms,
        doomed_rows,
        temp_bytes,
        resident_delta_bytes,
        unlinked_file_bytes,
    })
}

/// One timed Counting scan, leaving no TEMP table behind.
fn run_scan(conn: &Connection, doomed: DoomedTable, cutoff: &str) -> rusqlite::Result<f64> {
    conn.execute_batch(&format!("DROP TABLE IF EXISTS temp.{}", doomed.temp_name))?;
    let started = Instant::now();
    conn.execute(&doomed_scan_sql(doomed), params![cutoff])?;
    let elapsed = millis(started.elapsed());
    conn.execute_batch(&format!("DROP TABLE IF EXISTS temp.{}", doomed.temp_name))?;
    Ok(elapsed)
}

/// The same scan measured on an indexed and an unindexed copy of the corpus.
struct ScanComparison {
    table: &'static str,
    with_index_ms: f64,
    without_index_ms: f64,
}

impl ScanComparison {
    /// How much faster the scan runs once `idx_se_timestamp` is gone. Above
    /// 1.0 means the index was costing the scan time, not saving it.
    fn drop_speedup(&self) -> f64 {
        self.with_index_ms / self.without_index_ms.max(f64::MIN_POSITIVE)
    }
}

/// Compare one table's scan across the two copies.
///
/// Both copies are warmed first and the repetitions alternate between them,
/// because the naive ordering — all repetitions on one copy, then all on the
/// other — hands the second copy a hotter page cache and turns residency into
/// what looks like an index effect.
fn compare_scans(
    indexed: &Connection,
    unindexed: &Connection,
    doomed: DoomedTable,
    cutoff: &str,
) -> rusqlite::Result<ScanComparison> {
    run_scan(indexed, doomed, cutoff)?;
    run_scan(unindexed, doomed, cutoff)?;

    let mut with_index = Vec::with_capacity(SCAN_REPETITIONS);
    let mut without_index = Vec::with_capacity(SCAN_REPETITIONS);
    for _ in 0..SCAN_REPETITIONS {
        with_index.push(run_scan(indexed, doomed, cutoff)?);
        without_index.push(run_scan(unindexed, doomed, cutoff)?);
    }

    Ok(ScanComparison {
        table: doomed.target.as_str(),
        with_index_ms: median(&with_index),
        without_index_ms: median(&without_index),
    })
}

/// The `EXPLAIN QUERY PLAN` rows for a statement, joined into one line.
fn query_plan(conn: &Connection, sql: &str, cutoff: &str) -> rusqlite::Result<String> {
    let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
    let rows = statement.query_map(params![cutoff], |row| row.get::<_, String>(3))?;
    let mut detail = Vec::new();
    for row in rows {
        detail.push(row?);
    }
    Ok(detail.join(" | "))
}

/// What the chunked delete cost at one chunk size.
struct DeleteMeasurement {
    chunk_size: u64,
    chunks: u64,
    rows_deleted: u64,
    hold_ms: Vec<f64>,
    hold_ms_by_table: Vec<(&'static str, Vec<f64>)>,
    checkpoint_ms: Vec<f64>,
    chunk_wal: Vec<ChunkWal>,
    counting_ms: f64,
    delete_ms: f64,
    post_truncate_wal_bytes: u64,
    db_bytes_before: u64,
    db_bytes_after: u64,
}

/// WAL volume one committed chunk produced, paired with the rows that
/// produced it. The pair matters: the last chunk of a table is usually
/// partial, so dividing by the nominal chunk size would understate the
/// per-row constant the preflight needs.
struct ChunkWal {
    rows: u64,
    wal_bytes: u64,
}

impl DeleteMeasurement {
    fn total_ms(&self) -> f64 {
        self.counting_ms + self.delete_ms
    }

    fn max_chunk_wal_bytes(&self) -> u64 {
        self.chunk_wal
            .iter()
            .map(|chunk| chunk.wal_bytes)
            .max()
            .unwrap_or(0)
    }

    /// Worst per-row WAL cost observed across the run's *full* chunks.
    ///
    /// Partial chunks are excluded on purpose. A chunk's WAL is a fixed page
    /// overhead plus a per-row term, so the 700-row remainder chunk shows a
    /// per-row cost several times the real one. The preflight multiplies this
    /// constant by a whole chunk, so it has to be a whole chunk's rate.
    fn wal_bytes_per_row(&self) -> f64 {
        let full = self
            .chunk_wal
            .iter()
            .filter(|chunk| chunk.rows == self.chunk_size)
            .map(|chunk| chunk.wal_bytes as f64 / chunk.rows as f64)
            .fold(0.0_f64, f64::max);
        if full > 0.0 {
            return full;
        }
        // No chunk ever filled — fall back to whatever was measured rather
        // than publishing a zero.
        self.chunk_wal
            .iter()
            .filter(|chunk| chunk.rows > 0)
            .map(|chunk| chunk.wal_bytes as f64 / chunk.rows as f64)
            .fold(0.0_f64, f64::max)
    }
}

/// Samples collected while draining one table, kept together so the chunk
/// loop returns a new value rather than writing through borrowed buffers.
struct DrainSamples {
    rows_deleted: u64,
    hold_ms: Vec<f64>,
    checkpoint_ms: Vec<f64>,
    chunk_wal: Vec<ChunkWal>,
}

/// Drain one target table in chunks, exactly in the plan's shape: one scalar
/// boundary per chunk driving both the target delete and the bookkeeping
/// delete, inside a single transaction, with a truncating checkpoint after
/// each commit.
fn drain_table(
    conn: &mut Connection,
    db_path: &Path,
    doomed: DoomedTable,
    chunk_size: u64,
) -> Result<DrainSamples, Box<dyn std::error::Error>> {
    let boundary_sql = format!(
        "SELECT max(rid) FROM (SELECT rid FROM {} ORDER BY rid LIMIT ?1)",
        doomed.temp_name
    );
    let target_delete_sql = format!(
        "DELETE FROM {} WHERE rowid <= ?1 AND rowid IN (SELECT rid FROM {})",
        doomed.target.as_str(),
        doomed.temp_name
    );
    let bookkeeping_delete_sql = format!("DELETE FROM {} WHERE rid <= ?1", doomed.temp_name);

    let mut rows_deleted = 0_u64;
    let mut hold_ms = Vec::new();
    let mut checkpoint_ms = Vec::new();
    let mut chunk_wal = Vec::new();

    loop {
        let started = Instant::now();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let boundary: Option<i64> =
            tx.query_row(&boundary_sql, params![chunk_size as i64], |row| row.get(0))?;
        let Some(boundary) = boundary else {
            tx.rollback()?;
            break;
        };
        let deleted = tx.execute(&target_delete_sql, params![boundary])?;
        let cleared = tx.execute(&bookkeeping_delete_sql, params![boundary])?;
        tx.commit()?;
        hold_ms.push(millis(started.elapsed()));
        // Read WAL before the checkpoint: this is the dirty-page volume one
        // chunk produced, which is exactly what the preflight must budget.
        chunk_wal.push(ChunkWal {
            rows: deleted as u64,
            wal_bytes: wal_bytes(db_path),
        });

        let checkpoint_started = Instant::now();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        checkpoint_ms.push(millis(checkpoint_started.elapsed()));

        if cleared == 0 {
            let remaining: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM {}", doomed.temp_name),
                [],
                |row| row.get(0),
            )?;
            return Err(Box::new(SpikeError::ChunkStalled {
                table: doomed.target.as_str(),
                remaining: remaining.max(0) as u64,
            }));
        }
        rows_deleted += deleted as u64;
    }

    Ok(DrainSamples {
        rows_deleted,
        hold_ms,
        checkpoint_ms,
        chunk_wal,
    })
}

/// Run the whole Counting + delete pipeline against a private copy of the
/// corpus, so every chunk size is measured from the same starting state.
fn measure_delete(
    source_db: &Path,
    workspace: &Path,
    cutoff: &str,
    chunk_size: u64,
    temp_store: &'static str,
) -> Result<DeleteMeasurement, Box<dyn std::error::Error>> {
    let db_path = workspace.join(format!("retention-chunk-{chunk_size}.sqlite3"));
    std::fs::copy(source_db, &db_path)?;
    let db_bytes_before = std::fs::metadata(&db_path)?.len();

    let mut conn = open_maintenance(&db_path, temp_store)?;

    let counting_started = Instant::now();
    let mut doomed_counts = Vec::new();
    for doomed in DOOMED_TABLES {
        conn.execute(&doomed_scan_sql(doomed), params![cutoff])?;
        let rows: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {}", doomed.temp_name),
            [],
            |row| row.get(0),
        )?;
        doomed_counts.push(rows.max(0) as u64);
    }
    let counting_ms = millis(counting_started.elapsed());

    let mut hold_ms = Vec::new();
    let mut hold_ms_by_table = Vec::new();
    let mut checkpoint_ms = Vec::new();
    let mut chunk_wal = Vec::new();
    let mut rows_deleted = 0_u64;
    let delete_started = Instant::now();
    for (doomed, expected) in DOOMED_TABLES.iter().zip(doomed_counts.iter()) {
        let samples = drain_table(&mut conn, &db_path, *doomed, chunk_size)?;
        if samples.rows_deleted != *expected {
            return Err(Box::new(SpikeError::DeleteCountMismatch {
                table: doomed.target.as_str(),
                doomed: *expected,
                deleted: samples.rows_deleted,
            }));
        }
        rows_deleted += samples.rows_deleted;
        hold_ms.extend(samples.hold_ms.iter().copied());
        // Kept per table as well as pooled: `session_events` carries seven
        // indexes and `tool_actions` far fewer, so the pooled distribution is
        // bimodal and its percentiles describe neither table.
        hold_ms_by_table.push((doomed.target.as_str(), samples.hold_ms));
        checkpoint_ms.extend(samples.checkpoint_ms);
        chunk_wal.extend(samples.chunk_wal);
    }
    let delete_ms = millis(delete_started.elapsed());

    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let post_truncate_wal_bytes = wal_bytes(&db_path);
    drop(conn);
    let db_bytes_after = std::fs::metadata(&db_path)?.len();
    std::fs::remove_file(&db_path)?;

    Ok(DeleteMeasurement {
        chunk_size,
        chunks: hold_ms.len() as u64,
        rows_deleted,
        hold_ms,
        hold_ms_by_table,
        checkpoint_ms,
        chunk_wal,
        counting_ms,
        delete_ms,
        post_truncate_wal_bytes,
        db_bytes_before,
        db_bytes_after,
    })
}

/// Phase 0 — build the corpus and prove it matches its own plan.
fn report_corpus(
    fixture: &quill_lib::retention_fixture::RetentionFixture,
    months_retained: u32,
    build_wall_time: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let plan = fixture.plan();
    let conn = fixture.open_connection()?;
    println!("db_path={}", fixture.db_path().display());
    println!("build_wall_time_ms={}", build_wall_time.as_millis());
    println!("anchor={}", plan.boundary_timestamp(0));
    println!("months={}", plan.months());
    println!("sources={}", plan.sources());
    println!("months_retained={months_retained}");
    println!("cutoff={}", plan.boundary_timestamp(months_retained));

    for table in RetentionTable::ALL {
        for kind in RetentionRowKind::ALL {
            let planned = plan.total_rows(table, kind);
            let stored = count_rows(&conn, table, kind)?;
            // The spike's numbers are only comparable to the tests' numbers
            // if both ran on the same corpus, so a drift here is fatal, not a
            // warning to print past.
            if planned != stored {
                return Err(Box::new(SpikeError::CorpusDrift {
                    table: table.as_str(),
                    kind: kind.predicate(),
                    planned,
                    stored,
                }));
            }
            println!("rows.{}.{kind:?}={stored}", table.as_str());
        }
        println!(
            "doomed.{}={}",
            table.as_str(),
            plan.rows_before_boundary(months_retained, table, RetentionRowKind::OwnedConforming)
        );
    }
    // Leave the corpus checkpointed so every copy below starts from a single
    // self-contained file.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

/// Phase 5 — turn the measurements into the constants the delete engine and
/// its preflight are allowed to hard-code, and answer the Counting-phase
/// design question.
fn report_budgets(
    counting_by_store: &[CountingMeasurement],
    deletes: &[DeleteMeasurement],
    statvfs_us: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    // The recommended chunk is the largest swept size that still keeps a p95
    // transaction hold under the responsive-progress ceiling, because larger
    // chunks always finish the whole run sooner. If no size clears the
    // ceiling, the rule falls back to the most responsive size measured and
    // says so, rather than silently publishing a chunk nobody validated.
    let under_ceiling = deletes
        .iter()
        .rfind(|measurement| percentile(&measurement.hold_ms, 0.95) <= RESPONSIVE_CHUNK_HOLD_MS);
    let ceiling_met = under_ceiling.is_some();
    let chosen = match under_ceiling {
        Some(measurement) => measurement,
        None => deletes
            .iter()
            .min_by(|left, right| {
                percentile(&left.hold_ms, 0.95).total_cmp(&percentile(&right.hold_ms, 0.95))
            })
            .ok_or(SpikeError::NoChunkMeasurements)?,
    };

    let chosen_p95 = percentile(&chosen.hold_ms, 0.95);
    let chosen_mean = mean(&chosen.hold_ms);
    // The preflight must cover whichever `temp_store` is pinned, so the
    // per-row TEMP constant is the worse of the two measured settings.
    let temp_bytes_per_row = counting_by_store
        .iter()
        .map(CountingMeasurement::temp_bytes_per_row)
        .fold(0.0_f64, f64::max);
    let wal_bytes_per_row = chosen.wal_bytes_per_row();
    let recheck_interval = (FREE_SPACE_RECHECK_TARGET_MS / chosen_mean.max(1.0)).ceil() as u64;
    let recheck_window_us = chosen_mean * recheck_interval as f64 * 1_000.0;
    let counting_share = chosen.counting_ms / chosen.total_ms().max(f64::MIN_POSITIVE);

    println!("budget.rule.responsive_chunk_hold_ms={RESPONSIVE_CHUNK_HOLD_MS}");
    println!("budget.rule.headroom_multiplier={BUDGET_HEADROOM}");
    println!("budget.rule.ceiling_met={ceiling_met}");
    println!("budget.chunk_size={}", chosen.chunk_size);
    println!("budget.chunk_hold_p95_ms={chosen_p95:.1}");
    println!(
        "budget.per_chunk_wall_target_ms={:.0}",
        (chosen_p95 * BUDGET_HEADROOM).ceil()
    );
    println!("budget.wal_bytes_per_row={wal_bytes_per_row:.1}");
    println!("budget.temp_bytes_per_row={temp_bytes_per_row:.2}");
    // What the delete-phase preflight must actually require before chunk 0:
    // one chunk of WAL plus both doomed-rowid TEMP tables, before any safety
    // multiplier the engine adds on top.
    println!(
        "budget.preflight_chunk_wal_bytes={:.0}",
        wal_bytes_per_row * chosen.chunk_size as f64
    );
    println!(
        "budget.preflight_temp_bytes={:.0}",
        temp_bytes_per_row * chosen.rows_deleted as f64
    );
    println!("budget.free_space_recheck_interval_chunks={recheck_interval}");
    println!(
        "budget.free_space_recheck_cost_share={:.8}",
        statvfs_us / recheck_window_us.max(f64::MIN_POSITIVE)
    );
    println!(
        "budget.counting_phase_ms={:.0}",
        (chosen.counting_ms * BUDGET_HEADROOM).ceil()
    );
    println!(
        "budget.stale_preview_tolerance_ms={:.0}",
        (chosen.counting_ms * BUDGET_HEADROOM).ceil()
    );
    println!(
        "budget.total_wall_ms={:.0}",
        (chosen.total_ms() * BUDGET_HEADROOM).ceil()
    );
    println!("budget.rows_measured={}", chosen.rows_deleted);
    println!("budget.db_bytes_measured={}", chosen.db_bytes_before);

    println!("signal.counting_ms={:.1}", chosen.counting_ms);
    println!("signal.delete_ms={:.1}", chosen.delete_ms);
    println!("signal.counting_share_of_run={counting_share:.3}");
    let counting_dominates = counting_share >= 0.5;
    println!("signal.counting_dominates={counting_dominates}");
    println!(
        "signal.recommendation={}",
        if counting_dominates {
            "collapse-to-one-scan: preview_retention should take the lease and hand \
             run_retention_maintenance its materialized doomed set, because the second scan \
             costs more than the deletes it feeds"
        } else {
            "keep-two-scan-two-lease: the Counting phase is a minority of the run, so \
             rescanning under the run's own lease costs less than holding the lease across \
             the user's confirmation"
        }
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spec = RetentionFixtureSpec {
        months: env_u32("QUILL_RETENTION_SPIKE_MONTHS", SPIKE_MONTHS),
        owned_rows_per_month: env_u32(
            "QUILL_RETENTION_SPIKE_OWNED_ROWS_PER_MONTH",
            SPIKE_OWNED_ROWS_PER_MONTH,
        ),
        live_rows_per_month: env_u32(
            "QUILL_RETENTION_SPIKE_LIVE_ROWS_PER_MONTH",
            SPIKE_LIVE_ROWS_PER_MONTH,
        ),
        sources: env_u32("QUILL_RETENTION_SPIKE_SOURCES", SPIKE_SOURCES),
        ..RetentionFixtureSpec::default()
    };
    let months_retained = env_u32(
        "QUILL_RETENTION_SPIKE_MONTHS_RETAINED",
        SPIKE_MONTHS_RETAINED,
    );

    // ---- Phase 0: corpus -------------------------------------------------
    let build_started = Instant::now();
    let fixture = build_retention_fixture(&spec)?;
    let build_wall_time = build_started.elapsed();
    report_corpus(&fixture, months_retained, build_wall_time)?;

    let cutoff = fixture.plan().boundary_timestamp(months_retained);
    let db_path = fixture.db_path().to_path_buf();
    println!("db_bytes={}", std::fs::metadata(&db_path)?.len());

    let workspace = tempfile::tempdir()?;
    // SAFETY: process-global, set once from a single-threaded main before any
    // connection that could spill a temp file is opened. It only matters for
    // the `temp_store = FILE` pass, where it keeps the spill file on a path
    // this spike controls and can measure.
    unsafe {
        std::env::set_var("SQLITE_TMPDIR", workspace.path());
    }

    // ---- Phase 1: Counting under each candidate temp_store ---------------
    let mut counting_by_store = Vec::new();
    for temp_store in ["MEMORY", "FILE"] {
        let measurement = measure_counting(&db_path, &cutoff, temp_store)?;
        let store = measurement.temp_store;
        println!("counting.{store}.total_ms={:.1}", measurement.total_ms);
        for (table, elapsed) in &measurement.per_table_ms {
            println!("counting.{store}.scan_ms.{table}={elapsed:.1}");
        }
        println!("counting.{store}.doomed_rows={}", measurement.doomed_rows);
        println!("counting.{store}.temp_bytes={}", measurement.temp_bytes);
        println!(
            "counting.{store}.temp_bytes_per_doomed_row={:.2}",
            measurement.temp_bytes_per_row()
        );
        println!(
            "counting.{store}.resident_delta_bytes={}",
            measurement.resident_delta_bytes
        );
        println!(
            "counting.{store}.unlinked_open_file_bytes={}",
            measurement.unlinked_file_bytes
        );
        counting_by_store.push(measurement);
    }

    // ---- Phase 2: index sensitivity --------------------------------------
    let indexed_db = workspace.path().join("retention-indexed.sqlite3");
    let unindexed_db = workspace.path().join("retention-unindexed.sqlite3");
    std::fs::copy(&db_path, &indexed_db)?;
    std::fs::copy(&db_path, &unindexed_db)?;

    let indexed = open_maintenance(&indexed_db, "MEMORY")?;
    let unindexed = open_maintenance(&unindexed_db, "MEMORY")?;
    unindexed.execute_batch("DROP INDEX IF EXISTS idx_se_timestamp;")?;

    let mut comparisons = Vec::new();
    for doomed in DOOMED_TABLES {
        let table = doomed.target.as_str();
        let comparison = compare_scans(&indexed, &unindexed, doomed, &cutoff)?;
        println!(
            "scan.{table}.with_idx_se_timestamp_ms={:.1}",
            comparison.with_index_ms
        );
        println!(
            "scan.{table}.without_idx_se_timestamp_ms={:.1}",
            comparison.without_index_ms
        );
        println!("scan.{table}.drop_speedup={:.2}", comparison.drop_speedup());
        println!(
            "plan.{table}.with_idx_se_timestamp={}",
            query_plan(&indexed, &doomed_select_sql(doomed), &cutoff)?
        );
        println!(
            "plan.{table}.without_idx_se_timestamp={}",
            query_plan(&unindexed, &doomed_select_sql(doomed), &cutoff)?
        );
        comparisons.push(comparison);
    }
    // `tool_actions` cannot change plan when a `session_events` index is
    // dropped, so its ratio is pure measurement noise. Dividing the
    // `session_events` ratio by it leaves the part attributable to the index.
    let control = comparisons
        .iter()
        .find(|comparison| comparison.table == RetentionTable::ToolActions.as_str())
        .map(ScanComparison::drop_speedup)
        .unwrap_or(1.0);
    println!("scan.control_drop_speedup={control:.2}");
    for comparison in &comparisons {
        println!(
            "scan.{}.drop_speedup_control_normalized={:.2}",
            comparison.table,
            comparison.drop_speedup() / control.max(f64::MIN_POSITIVE)
        );
    }
    drop(indexed);
    drop(unindexed);
    std::fs::remove_file(&indexed_db)?;
    std::fs::remove_file(&unindexed_db)?;

    // ---- Phase 3: chunked delete sweep -----------------------------------
    let mut deletes = Vec::new();
    for chunk_size in CHUNK_SIZES {
        let measurement =
            measure_delete(&db_path, workspace.path(), &cutoff, chunk_size, "MEMORY")?;
        let holds = &measurement.hold_ms;
        println!("delete.{chunk_size}.chunks={}", measurement.chunks);
        println!(
            "delete.{chunk_size}.rows_deleted={}",
            measurement.rows_deleted
        );
        println!(
            "delete.{chunk_size}.counting_ms={:.1}",
            measurement.counting_ms
        );
        println!("delete.{chunk_size}.delete_ms={:.1}", measurement.delete_ms);
        println!("delete.{chunk_size}.total_ms={:.1}", measurement.total_ms());
        println!(
            "delete.{chunk_size}.hold_ms.min={:.1}",
            percentile(holds, 0.0)
        );
        println!("delete.{chunk_size}.hold_ms.mean={:.1}", mean(holds));
        println!(
            "delete.{chunk_size}.hold_ms.p95={:.1}",
            percentile(holds, 0.95)
        );
        println!(
            "delete.{chunk_size}.hold_ms.max={:.1}",
            percentile(holds, 1.0)
        );
        for (table, table_holds) in &measurement.hold_ms_by_table {
            println!(
                "delete.{chunk_size}.hold_ms.{table}.mean={:.1}",
                mean(table_holds)
            );
            println!(
                "delete.{chunk_size}.hold_ms.{table}.p95={:.1}",
                percentile(table_holds, 0.95)
            );
            println!(
                "delete.{chunk_size}.hold_ms.{table}.max={:.1}",
                percentile(table_holds, 1.0)
            );
        }
        println!(
            "delete.{chunk_size}.checkpoint_ms.mean={:.1}",
            mean(&measurement.checkpoint_ms)
        );
        println!(
            "delete.{chunk_size}.wal_bytes.max={}",
            measurement.max_chunk_wal_bytes()
        );
        println!(
            "delete.{chunk_size}.wal_bytes_per_row={:.1}",
            measurement.wal_bytes_per_row()
        );
        println!(
            "delete.{chunk_size}.post_truncate_wal_bytes={}",
            measurement.post_truncate_wal_bytes
        );
        println!(
            "delete.{chunk_size}.db_bytes_before={}",
            measurement.db_bytes_before
        );
        println!(
            "delete.{chunk_size}.db_bytes_after={}",
            measurement.db_bytes_after
        );
        deletes.push(measurement);
    }

    // ---- Phase 4: free-space probe cost ----------------------------------
    let probe_started = Instant::now();
    let mut free_space = 0_u64;
    for _ in 0..FREE_SPACE_PROBE_SAMPLES {
        free_space = available_disk_space(workspace.path())?;
    }
    let statvfs_us =
        probe_started.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(FREE_SPACE_PROBE_SAMPLES);
    println!("free_space.available_bytes={free_space}");
    println!("free_space.statvfs_us={statvfs_us:.2}");

    // ---- Phase 5: derived budgets and design signal ----------------------
    report_budgets(&counting_by_store, &deletes, statvfs_us)
}
