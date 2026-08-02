//! Chunked delete engine and delete-phase preflight (feature 014, Phase 2).
//!
//! This is the destructive core of retention pruning: a dedicated maintenance
//! connection, a one-pass doomed-rowid scan into two `TEMP TABLE`s, a
//! disk/WAL/TEMP preflight, and a bounded chunked delete that commits per
//! chunk and truncates the WAL between chunks. It owns no policy — the value
//! grammars, the cutoff and the monotonic watermark rule live in
//! [`crate::retention`] — and it owns no UI: progress, phases and the composite
//! command's result shape belong to their own items.
//!
//! # Why the work is shaped this way
//!
//! * **A dedicated connection.** The primary connection is a single
//!   process-wide mutex, so scanning and deleting on it would block every read
//!   IPC — the always-on-top widget included — for the whole run. On its own
//!   WAL connection, readers keep reading and only writes are gated, by the
//!   quiesce lease the caller already holds.
//! * **`PRAGMA temp_store = MEMORY`, pinned.** The timing spike measured the
//!   same 7.4 MB doomed-rowid b-tree under both settings at identical wall
//!   time, so the only question was where those bytes land. `FILE` puts them on
//!   a temp filesystem the disk preflight may not even have measured; `MEMORY`
//!   costs under 10 MB of RSS for a 700k-row prune. The pragma is set
//!   explicitly rather than inherited, because the build default is not a
//!   decision anybody made.
//! * **One scan, not one scan per chunk.** `tool_actions` has no index leading
//!   with `timestamp`, so a naive per-chunk `WHERE timestamp < ?` would rescan
//!   the table on every chunk. The single pass into `retention_doomed_*`
//!   yields the exact count for free, makes every chunk a rowid seek, and
//!   freezes the delete set so the result reported is the set that was
//!   previewed.
//! * **A scalar chunk boundary, not two `LIMIT` scans.** Two unordered
//!   `SELECT … LIMIT` scans over the same temp table carry no guarantee of
//!   agreeing, so each chunk transaction materializes one `:max` rowid and
//!   drives both the target delete and the bookkeeping delete from it. The
//!   target table and its temp table therefore cannot diverge.
//! * **A checkpoint between chunks.** Deleting one row rewrites its entry in
//!   every surviving index, so WAL churn is index-dominated. Under the quiesce
//!   lease there are no competing writers, so
//!   `PRAGMA wal_checkpoint(TRUNCATE)` after each commit holds WAL at one
//!   chunk instead of the whole run — measured at **zero** post-checkpoint
//!   bytes at every swept chunk size.
//!
//! # Budgets
//!
//! Every numeric constant below comes from the retention timing spike
//! (`specs/014-retention-pruning/retention-timing-spike.md`) and none of them
//! was chosen here. They are a ceiling derived from a measurement, not a
//! threshold this module can fail against.

// The delete engine is deliberately shipped ahead of its consumers: the
// composite command and the preview command are separate items that call into
// it, and pinning the destructive invariants at this layer — with their own
// tests — is the point of landing it first.
#![allow(dead_code)]

use std::fs;
use std::io::{BufWriter, Write};
use std::os::raw::c_int;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde_json::{Map, Value, json};

use crate::retention::{
    RetentionAuditRecord, RetentionRunStatus, RetentionTableCounts, is_conforming_timestamp,
};
use crate::storage::Storage;

/// Rows deleted per chunk transaction.
///
/// The largest swept size whose pooled p95 transaction hold stays under one
/// second — the visible-progress threshold for a background job, not the
/// instantaneous-response one. Smaller chunks buy finer progress with real
/// wall time: the same 701,400-row delete takes 29.5 s at 5,000 rows and
/// 13.5 s at 25,000.
pub const RETENTION_CHUNK_ROWS: u64 = 25_000;

/// WAL bytes one deleted row produces, at [`RETENTION_CHUNK_ROWS`].
///
/// Measured as the worst full chunk's WAL divided by its rows. The per-row
/// figure falls as a chunk's fixed page overhead amortizes, so this is
/// deliberately the rate at the chunk size the preflight actually multiplies.
pub const RETENTION_WAL_BYTES_PER_ROW: f64 = 788.7;

/// Bytes of doomed-rowid b-tree per doomed row.
///
/// An 8-byte rowid plus ~38% b-tree overhead, identical under both
/// `temp_store` settings.
pub const RETENTION_TEMP_BYTES_PER_DOOMED_ROW: f64 = 11.05;

/// How many chunks may commit between free-space re-checks.
///
/// A `statvfs` call costs 3.16 µs against a mean chunk hold of 417.7 ms, so
/// three chunks of guarded window costs 2.5 × 10⁻⁶ of that window.
pub const RETENTION_FREE_SPACE_RECHECK_CHUNKS: u32 = 3;

/// Headroom multiplier applied on top of the two measured preflight terms.
///
/// The spike deliberately left this to the engine: its own ×3 headroom turns a
/// measurement on one machine into a ceiling a slower machine still fits
/// under, whereas this multiplier answers a different question — how much of
/// the disk another process may consume while the run is in flight. Doubling
/// a ~27 MB requirement is free on any disk that can hold the database at all.
pub const RETENTION_PREFLIGHT_SAFETY_MULTIPLIER: u64 = 2;

/// Skip reason for a scan that found nothing to delete.
pub const RETENTION_NOTHING_OLDER_REASON: &str =
    "Nothing older than the retention cutoff remains to delete";

/// Virtual-machine instructions between progress-handler invocations.
///
/// Small enough that the heartbeat fires many times per second on the
/// Counting scan, large enough that the callback is not itself a cost.
const SCAN_HEARTBEAT_VM_OPS: c_int = 20_000;

/// Wall-clock cadence at which the Counting heartbeat nudges its percentage.
///
/// The Counting phase is one opaque `CREATE TEMP TABLE … AS SELECT` that emits
/// no rows, and on a production corpus it runs for the better part of a
/// second. Reporting a pinned `0%` for that long reads as a hang, so the
/// percentage is advanced on wall time rather than on progress the statement
/// cannot report.
const SCAN_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(150);

/// Highest percentage the heartbeat will climb to on its own.
///
/// The heartbeat is a liveness signal, not a measurement, so it must never
/// reach 100 and claim a completion it cannot observe. Each table's real
/// completion snaps its half of the range shut.
const SCAN_HEARTBEAT_CEILING: u8 = 45;

/// One retention target table and its doomed-rowid companion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RetentionTarget {
    ToolActions,
    SessionEvents,
    ModelUsageObservations,
}

impl RetentionTarget {
    /// Both targets, in the order the run drains them.
    pub const ALL: [RetentionTarget; 3] = [
        RetentionTarget::ToolActions,
        RetentionTarget::SessionEvents,
        RetentionTarget::ModelUsageObservations,
    ];

    /// Targets covered by the transcript fixture. Model observations have
    /// their own source lifecycle and are exercised through storage tests.
    #[cfg(test)]
    pub const TRANSCRIPT_TARGETS: [RetentionTarget; 2] =
        [RetentionTarget::ToolActions, RetentionTarget::SessionEvents];

    /// SQL name of the table rows are deleted from.
    pub const fn table(self) -> &'static str {
        match self {
            RetentionTarget::ToolActions => "tool_actions",
            RetentionTarget::SessionEvents => "session_events",
            RetentionTarget::ModelUsageObservations => "model_usage_observations",
        }
    }

    /// SQL name of the `TEMP TABLE` holding this target's doomed rowids.
    pub const fn doomed_table(self) -> &'static str {
        match self {
            RetentionTarget::ToolActions => "retention_doomed_tool_actions",
            RetentionTarget::SessionEvents => "retention_doomed_session_events",
            RetentionTarget::ModelUsageObservations => "retention_doomed_model_usage_observations",
        }
    }

    /// Read this target's field out of a per-table count pair.
    pub const fn counts_field(self, counts: &RetentionTableCounts) -> i64 {
        match self {
            RetentionTarget::ToolActions => counts.tool_actions,
            RetentionTarget::SessionEvents => counts.session_events,
            RetentionTarget::ModelUsageObservations => counts.model_usage_observations,
        }
    }

    /// A **new** count pair with this target's field replaced.
    pub const fn with_count(
        self,
        counts: &RetentionTableCounts,
        value: i64,
    ) -> RetentionTableCounts {
        match self {
            RetentionTarget::ToolActions => RetentionTableCounts {
                tool_actions: value,
                session_events: counts.session_events,
                model_usage_observations: counts.model_usage_observations,
            },
            RetentionTarget::SessionEvents => RetentionTableCounts {
                tool_actions: counts.tool_actions,
                session_events: value,
                model_usage_observations: counts.model_usage_observations,
            },
            RetentionTarget::ModelUsageObservations => RetentionTableCounts {
                tool_actions: counts.tool_actions,
                session_events: counts.session_events,
                model_usage_observations: value,
            },
        }
    }
}

/// Faults the delete engine refuses to paper over.
///
/// Everything a *user* can hit and recover from — no free disk, a run that
/// stopped part-way — is a status on the report rather than an error. These
/// variants are the cases where the caller is holding something broken:
/// a cutoff that cannot be compared, a database that will not open, SQL that
/// failed, or a chunk loop that is not making progress.
#[derive(Debug)]
pub enum RetentionDeleteError {
    /// A cutoff that is not byte-comparable against stored timestamps. The
    /// scan's guard and the insert filter's guard must agree exactly, so a
    /// cutoff that cannot be ordered is refused rather than approximated.
    MalformedCutoff { cutoff: String },
    /// The dedicated maintenance connection could not be opened or configured.
    Connection { reason: String },
    /// A statement failed on the maintenance connection.
    Sqlite(rusqlite::Error),
    /// The watermark advance failed, so the run refuses to delete anything —
    /// deleting without a durable watermark re-opens the resurrection path the
    /// watermark exists to close.
    WatermarkAdvance { reason: String },
    /// A chunk committed no bookkeeping rows while its temp table was not
    /// empty, which would spin forever.
    ChunkStalled { table: &'static str, remaining: u64 },
    /// The audit record could not be persisted.
    AuditWrite { reason: String },
}

impl std::fmt::Display for RetentionDeleteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetentionDeleteError::MalformedCutoff { cutoff } => write!(
                f,
                "Retention cutoff {cutoff:?} is not a conforming timestamp (24 characters ending in Z)"
            ),
            RetentionDeleteError::Connection { reason } => {
                write!(f, "Open retention maintenance connection: {reason}")
            }
            RetentionDeleteError::Sqlite(error) => {
                write!(f, "Retention maintenance statement failed: {error}")
            }
            RetentionDeleteError::WatermarkAdvance { reason } => {
                write!(
                    f,
                    "Advance retention watermark before first chunk: {reason}"
                )
            }
            RetentionDeleteError::ChunkStalled { table, remaining } => write!(
                f,
                "Retention chunk loop for {table} stalled with {remaining} doomed rowids remaining"
            ),
            RetentionDeleteError::AuditWrite { reason } => {
                write!(f, "Persist retention audit record: {reason}")
            }
        }
    }
}

impl std::error::Error for RetentionDeleteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RetentionDeleteError::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for RetentionDeleteError {
    fn from(error: rusqlite::Error) -> Self {
        RetentionDeleteError::Sqlite(error)
    }
}

/// Why the delete-phase preflight refused to start.
///
/// This is a *skip*, not an error: nothing was deleted, the watermark is
/// untouched, and the caller reports a reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetentionPreflightFailure {
    /// Free space could not be read at all.
    Unreadable { reason: String },
    /// Free space is below the delete-phase budget.
    InsufficientSpace {
        required_bytes: u64,
        available_bytes: u64,
    },
}

impl std::fmt::Display for RetentionPreflightFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetentionPreflightFailure::Unreadable { reason } => write!(
                f,
                "Could not check free disk space before removing rows: {reason}"
            ),
            RetentionPreflightFailure::InsufficientSpace {
                required_bytes,
                available_bytes,
            } => write!(
                f,
                "Insufficient free disk space to remove rows: need {required_bytes} bytes, \
                 have {available_bytes} bytes"
            ),
        }
    }
}

impl std::error::Error for RetentionPreflightFailure {}

/// The delete phase's disk requirement, itemized.
///
/// Kept as three numbers rather than one so a failing preflight can say which
/// term dominated, and so a test can fail the delete budget without touching
/// the VACUUM budget — they are genuinely different checks with genuinely
/// different magnitudes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionDeleteBudget {
    /// One chunk's estimated WAL, from [`RETENTION_WAL_BYTES_PER_ROW`].
    pub wal_bytes: u64,
    /// Both doomed-rowid temp tables, from
    /// [`RETENTION_TEMP_BYTES_PER_DOOMED_ROW`].
    pub temp_bytes: u64,
    /// `(wal + temp) × `[`RETENTION_PREFLIGHT_SAFETY_MULTIPLIER`].
    pub required_bytes: u64,
}

impl RetentionDeleteBudget {
    /// Price a delete phase over `doomed_rows` rows at `chunk_rows` per chunk.
    ///
    /// The temp term is held against **disk** even though `temp_store` is
    /// pinned to `MEMORY`, where those bytes are RSS rather than filesystem
    /// bytes. Two reasons: at 11.05 B per doomed row the term is a rounding
    /// error beside the WAL term, so carrying it costs nothing; and it keeps
    /// the requirement correct if the pinned `temp_store` is ever revisited,
    /// which is exactly the kind of change that silently invalidates a budget
    /// computed the other way.
    pub fn estimate(doomed_rows: u64, chunk_rows: u64) -> Self {
        let chunk_rows = chunk_rows.max(1).min(doomed_rows.max(1));
        let wal_bytes = (chunk_rows as f64 * RETENTION_WAL_BYTES_PER_ROW).ceil() as u64;
        let temp_bytes = (doomed_rows as f64 * RETENTION_TEMP_BYTES_PER_DOOMED_ROW).ceil() as u64;
        let required_bytes = wal_bytes
            .saturating_add(temp_bytes)
            .saturating_mul(RETENTION_PREFLIGHT_SAFETY_MULTIPLIER);
        Self {
            wal_bytes,
            temp_bytes,
            required_bytes,
        }
    }
}

/// What the one-pass scan found, per table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetentionScanReport {
    /// Rows the chunked delete will remove.
    pub doomed: RetentionTableCounts,
    /// Owned rows older than the cutoff whose timestamp failed the conformance
    /// guard, and which are therefore retained and reported rather than
    /// deleted.
    pub nonconforming: RetentionTableCounts,
}

/// Durable JSONL copy written before a retention delete starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionArchiveReport {
    /// Absolute path of the completed sidecar.
    pub path: PathBuf,
    /// Every row represented by the preview: deletion candidates plus
    /// pre-cutoff rows retained because their timestamps do not conform.
    pub rows: RetentionTableCounts,
}

impl RetentionScanReport {
    /// Total rows all three temp tables hold.
    pub const fn total_doomed(&self) -> i64 {
        self.doomed.tool_actions + self.doomed.session_events + self.doomed.model_usage_observations
    }

    /// Structured reason a run with this scan result must skip, or `None` when
    /// there is work to do.
    pub const fn skip_reason(&self) -> Option<&'static str> {
        if self.total_doomed() == 0 {
            Some(RETENTION_NOTHING_OLDER_REASON)
        } else {
            None
        }
    }
}

/// What one committed chunk did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionChunkReport {
    pub target: RetentionTarget,
    /// 1-based across the whole run, not per table.
    pub chunk_index: u32,
    pub rows_deleted: u64,
    /// Doomed rowids still queued for this target after the chunk committed.
    pub rows_remaining: u64,
}

/// Whether the chunk loop continues after a committed chunk.
///
/// [`RetentionChunkControl::Interrupt`] exists so a test can stop a run
/// between chunks the way a process kill would, and prove that committed
/// chunks stay committed and the next run needs no special handling. Nothing
/// in the production path installs a hook, so nothing in the production path
/// can produce it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetentionChunkControl {
    Continue,
    Interrupt,
}

/// Free bytes on the filesystem holding a path.
pub type FreeSpaceProbe<'a> = &'a dyn Fn(&Path) -> Result<u64, String>;

/// Decides whether the chunk loop continues after a committed chunk.
pub type ChunkHook<'a> = &'a dyn Fn(&RetentionChunkReport) -> RetentionChunkControl;

/// Percentage sink for the delete phase.
pub type DeleteProgressSink<'a> = &'a dyn Fn(u8);

/// Percentage sink for the optional archive phase.
pub type ArchiveProgressSink<'a> = &'a dyn Fn(u8);

/// Percentage sink shared with the `'static` Counting-phase progress handler.
pub type ScanProgressSink = Arc<dyn Fn(u8) + Send + Sync>;

/// Injection points for the chunked delete.
///
/// Defaults are the production behaviour: the real `statvfs`, no hook, no
/// progress sink, and the spike's chunk size and re-check interval. Tests
/// override individual fields; nothing here changes what SQL runs.
pub struct RetentionDeleteControls<'a> {
    /// Rows per chunk transaction.
    pub chunk_rows: u64,
    /// Chunks between free-space re-checks.
    pub free_space_recheck_chunks: u32,
    /// `None` uses `statvfs`.
    pub free_space: Option<FreeSpaceProbe<'a>>,
    /// Called after every committed chunk, before the next one starts.
    pub after_chunk: Option<ChunkHook<'a>>,
    /// Counting-phase heartbeat, 0–100. Shared with the progress handler, so
    /// it must be cheap and must not touch the database.
    pub scan_progress: Option<ScanProgressSink>,
    /// Delete-phase percentage, 0–100, emitted once per committed chunk.
    pub delete_progress: Option<DeleteProgressSink<'a>>,
    /// When present, write the preview-counted rows here before any delete.
    pub archive_directory: Option<&'a Path>,
    /// Archive-phase percentage, 0–100.
    pub archive_progress: Option<ArchiveProgressSink<'a>>,
}

impl Default for RetentionDeleteControls<'_> {
    fn default() -> Self {
        Self {
            chunk_rows: RETENTION_CHUNK_ROWS,
            free_space_recheck_chunks: RETENTION_FREE_SPACE_RECHECK_CHUNKS,
            free_space: None,
            after_chunk: None,
            scan_progress: None,
            delete_progress: None,
            archive_directory: None,
            archive_progress: None,
        }
    }
}

impl RetentionDeleteControls<'_> {
    fn probe_free_space(&self, path: &Path) -> Result<u64, String> {
        match self.free_space {
            Some(probe) => probe(path),
            None => available_disk_space(path),
        }
    }

    fn recheck_interval(&self) -> u32 {
        self.free_space_recheck_chunks.max(1)
    }

    fn chunk_rows(&self) -> u64 {
        self.chunk_rows.max(1)
    }
}

/// The consent-bound inputs of one delete phase.
///
/// `cutoff` is the token the preview produced and the user approved; the
/// engine uses it verbatim for the scan, the deletes, the watermark advance
/// and the audit record, and never re-derives it.
#[derive(Clone, Debug)]
pub struct RetentionDeleteRequest {
    pub cutoff: String,
    pub window_days: i64,
    /// Whole-file bytes before the run, carried into the audit record.
    pub bytes_before: u64,
    /// Instant stamped on the audit record.
    pub ran_at: DateTime<Utc>,
}

/// Everything the delete phase produced, including the record it persisted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionDeletePhaseReport {
    pub status: RetentionRunStatus,
    /// Populated for a skip.
    pub reason: Option<String>,
    /// Populated if and only if `status` is
    /// [`RetentionRunStatus::Partial`].
    pub error_reason: Option<String>,
    /// Rows the scan found. Equal to `deleted` on a completed run.
    pub doomed: RetentionTableCounts,
    pub deleted: RetentionTableCounts,
    pub nonconforming: RetentionTableCounts,
    /// The delete phase's own disk requirement, `None` when the scan found
    /// nothing and the preflight was therefore never priced.
    pub budget: Option<RetentionDeleteBudget>,
    /// The record written to `retention.last_run` on this path.
    pub audit: RetentionAuditRecord,
    /// Whether the watermark reached the cutoff during this call.
    pub watermark_advanced: bool,
    /// Completed sidecar, when the caller requested one.
    pub archive: Option<RetentionArchiveReport>,
}

/// Free bytes on the filesystem holding `path`.
#[cfg(unix)]
pub(crate) fn available_disk_space(path: &Path) -> Result<u64, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "Database directory contains an unsupported NUL byte".to_string())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is NUL-terminated and `stats` points to writable
    // storage sized for the struct.
    let result = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(format!(
            "Read free disk space for the retention delete phase: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `statvfs` returned 0, so it initialized the struct.
    let stats = unsafe { stats.assume_init() };
    stats
        .f_bavail
        .checked_mul(stats.f_frsize)
        .ok_or_else(|| "Free disk space value overflowed the delete-phase preflight".to_string())
}

#[cfg(not(unix))]
pub(crate) fn available_disk_space(_path: &Path) -> Result<u64, String> {
    Err("The retention delete-phase preflight is unavailable on this platform".to_string())
}

/// The one-pass doomed-rowid scan, for one target table.
///
/// The two guards are load-bearing and neither is optional. `source_key IS NOT
/// NULL` excludes live rows, which no retention path may ever touch;
/// `length(timestamp) = 24 AND timestamp LIKE '%Z'` excludes timestamps that
/// are not byte-comparable, whose `+` sorts before `.` and would therefore
/// mis-compare at the boundary. This guard is the delete-side half of a
/// symmetry with the insert filter: a row this scan refuses to delete is a row
/// the insert filter must refuse to suppress.
fn doomed_scan_sql(target: RetentionTarget) -> String {
    // Both interpolated fragments are compile-time constants owned by
    // `RetentionTarget`; nothing caller-supplied reaches the SQL text.
    match target {
        RetentionTarget::ModelUsageObservations => format!(
            "CREATE TEMP TABLE {} AS
             SELECT rowid AS rid FROM {} WHERE observed_at_ms < ?1",
            target.doomed_table(),
            target.table()
        ),
        _ => format!(
            "CREATE TEMP TABLE {} AS
             SELECT rowid AS rid FROM {}
              WHERE source_key IS NOT NULL
                AND length(timestamp) = 24 AND timestamp LIKE '%Z'
                AND timestamp < ?1",
            target.doomed_table(),
            target.table()
        ),
    }
}

/// Owned pre-cutoff rows that failed the conformance guard.
///
/// The comparison is a plain byte order applied to a value the guard has
/// already declared un-orderable, which is acceptable precisely because this
/// number is a *report* and never a delete predicate: it answers "how many
/// old-looking rows did the guard keep", and no row is removed on its basis.
fn nonconforming_count_sql(target: RetentionTarget) -> String {
    if target == RetentionTarget::ModelUsageObservations {
        return "SELECT 0".to_string();
    }
    format!(
        "SELECT COUNT(*) FROM {}
          WHERE source_key IS NOT NULL
            AND NOT (length(timestamp) = 24 AND timestamp LIKE '%Z')
            AND timestamp < ?1",
        target.table()
    )
}

/// Every source-owned row in a target table, regardless of age.
fn owned_count_sql(target: RetentionTarget) -> String {
    if target == RetentionTarget::ModelUsageObservations {
        return "SELECT COUNT(*) FROM model_usage_observations".to_string();
    }
    format!(
        "SELECT COUNT(*) FROM {} WHERE source_key IS NOT NULL",
        target.table()
    )
}

/// Count the source-owned rows both target tables hold, at any age.
///
/// This exists for the preview, which has to tell two zero-row outcomes apart
/// that a run has no reason to distinguish: a database with no owned rows at
/// all — where "nothing older than the cutoff" is a misleading thing to say —
/// and a populated database whose rows are simply all newer than the cutoff.
///
/// It also closes the partition the scan opens. Owned rows are exactly the
/// doomed set, plus the pre-cutoff rows the conformance guard kept, plus
/// everything at or after the cutoff, so a preview derives "the cutoff covers
/// every owned row" by subtraction instead of paying for a third full pass
/// over both tables.
pub fn count_owned_rows(conn: &Connection) -> Result<RetentionTableCounts, RetentionDeleteError> {
    let mut counts = RetentionTableCounts::default();
    for target in RetentionTarget::ALL {
        let rows: i64 = conn.query_row(&owned_count_sql(target), [], |row| row.get(0))?;
        counts = target.with_count(&counts, rows.max(0));
    }
    Ok(counts)
}

/// Open the dedicated maintenance connection, pragmas pinned.
///
/// The caller **must** drop the returned connection before invoking
/// `vacuum_database`: it owns both `retention_doomed_*` temp tables, and
/// VACUUM rebuilds the whole file without tolerating another connection
/// holding schema-visible temp state.
pub fn open_maintenance_connection(db_path: &Path) -> Result<Connection, RetentionDeleteError> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| RetentionDeleteError::Connection {
        reason: error.to_string(),
    })?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(|error| {
        RetentionDeleteError::Connection {
            reason: error.to_string(),
        }
    })?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;",
    )
    .map_err(|error| RetentionDeleteError::Connection {
        reason: error.to_string(),
    })?;
    Ok(conn)
}

/// Materialize both doomed-rowid temp tables and count what the guard kept.
///
/// A `progress_handler` is installed for the duration and uninstalled before
/// returning, so the delete phase — which has real per-chunk progress — never
/// runs under a heartbeat.
pub fn scan_doomed_rows(
    conn: &Connection,
    cutoff: &str,
    controls: &RetentionDeleteControls<'_>,
) -> Result<RetentionScanReport, RetentionDeleteError> {
    if !is_conforming_timestamp(cutoff) {
        return Err(RetentionDeleteError::MalformedCutoff {
            cutoff: cutoff.to_string(),
        });
    }

    let cutoff_ms = DateTime::parse_from_rfc3339(cutoff)
        .map_err(|_| RetentionDeleteError::MalformedCutoff {
            cutoff: cutoff.to_string(),
        })?
        .timestamp_millis();
    let mut doomed = RetentionTableCounts::default();
    let mut nonconforming = RetentionTableCounts::default();

    for (index, target) in RetentionTarget::ALL.into_iter().enumerate() {
        // Each table owns a third of the bar, so the phase advances across the whole
        // scan rather than restarting at zero for the second table.
        let target_count = RetentionTarget::ALL.len();
        let base = (index * 100 / target_count) as u8;
        let next = (((index + 1) * 100 / target_count).min(100)) as u8;
        install_scan_heartbeat(conn, controls, base);

        conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS temp.{}",
            target.doomed_table()
        ))?;
        match target {
            RetentionTarget::ModelUsageObservations => {
                conn.execute(&doomed_scan_sql(target), params![cutoff_ms])?;
            }
            _ => {
                conn.execute(&doomed_scan_sql(target), params![cutoff])?;
            }
        }

        let rows: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM temp.{}", target.doomed_table()),
            [],
            |row| row.get(0),
        )?;
        let skipped: i64 = match target {
            RetentionTarget::ModelUsageObservations => {
                conn.query_row(&nonconforming_count_sql(target), [], |row| row.get(0))?
            }
            _ => conn.query_row(&nonconforming_count_sql(target), params![cutoff], |row| {
                row.get(0)
            })?,
        };

        doomed = target.with_count(&doomed, rows.max(0));
        nonconforming = target.with_count(&nonconforming, skipped.max(0));

        clear_scan_heartbeat(conn);
        report_scan_progress(controls, next);
    }

    Ok(RetentionScanReport {
        doomed,
        nonconforming,
    })
}

/// Stream every preview-counted row to an atomic JSONL sidecar.
///
/// The archive predicate is the union already reported by the preview:
/// conforming deletion candidates plus non-conforming source-owned rows whose
/// byte value sorts before the cutoff. The latter remain in SQLite, but are
/// included because the preview counted them and an archive claiming to cover
/// that preview must not silently omit a class.
fn write_retention_archive(
    conn: &Connection,
    directory: &Path,
    request: &RetentionDeleteRequest,
    scan: &RetentionScanReport,
    progress: Option<ArchiveProgressSink<'_>>,
) -> Result<RetentionArchiveReport, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("Create retention archive directory: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Protect retention archive directory: {error}"))?;
    }

    let mut temporary = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| format!("Create retention archive file: {error}"))?;
    let nonce = temporary
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sidecar")
        .trim_start_matches('.');
    let stamp = request
        .ran_at
        .to_rfc3339_opts(SecondsFormat::Millis, true)
        .replace([':', '-', '.'], "");
    let final_path = directory.join(format!("quill-retention-archive-{stamp}-{nonce}.jsonl"));

    let expected = RetentionTableCounts {
        tool_actions: scan.doomed.tool_actions + scan.nonconforming.tool_actions,
        session_events: scan.doomed.session_events + scan.nonconforming.session_events,
        model_usage_observations: scan.doomed.model_usage_observations
            + scan.nonconforming.model_usage_observations,
    };
    let expected_total =
        expected.tool_actions + expected.session_events + expected.model_usage_observations;
    let created_at = request.ran_at.to_rfc3339_opts(SecondsFormat::Millis, true);

    if let Some(sink) = progress {
        sink(0);
    }

    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer(
            &mut writer,
            &json!({
                "record_type": "manifest",
                "schema": 1,
                "created_at": created_at,
                "cutoff": request.cutoff,
                "window_days": request.window_days,
                "rows": {
                    "tool_actions": expected.tool_actions,
                    "session_events": expected.session_events,
                    "model_usage_observations": expected.model_usage_observations,
                },
                "delete_candidates": {
                    "tool_actions": scan.doomed.tool_actions,
                    "session_events": scan.doomed.session_events,
                    "model_usage_observations": scan.doomed.model_usage_observations,
                },
                "nonconforming_retained": {
                    "tool_actions": scan.nonconforming.tool_actions,
                    "session_events": scan.nonconforming.session_events,
                    "model_usage_observations": scan.nonconforming.model_usage_observations,
                },
            }),
        )
        .map_err(|error| format!("Serialize retention archive manifest: {error}"))?;
        writer
            .write_all(b"\n")
            .map_err(|error| format!("Write retention archive manifest: {error}"))?;

        let mut written = RetentionTableCounts::default();
        let mut written_total = 0_i64;
        let cutoff_ms = DateTime::parse_from_rfc3339(&request.cutoff)
            .map_err(|error| format!("Parse retention archive cutoff: {error}"))?
            .timestamp_millis();
        for target in RetentionTarget::ALL {
            let sql = match target {
                RetentionTarget::ModelUsageObservations => format!(
                    "SELECT rowid AS archive_rowid, *
                       FROM {}
                      WHERE observed_at_ms < ?1
                      ORDER BY rowid",
                    target.table()
                ),
                _ => format!(
                    "SELECT rowid AS archive_rowid, *
                       FROM {}
                      WHERE source_key IS NOT NULL
                        AND timestamp < ?1
                      ORDER BY rowid",
                    target.table()
                ),
            };
            let mut statement = conn
                .prepare(&sql)
                .map_err(|error| format!("Prepare {} archive query: {error}", target.table()))?;
            let column_names = statement
                .column_names()
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            let timestamp_index = if target == RetentionTarget::ModelUsageObservations {
                None
            } else {
                Some(
                    column_names
                        .iter()
                        .position(|name| name == "timestamp")
                        .ok_or_else(|| {
                            format!("{} archive query omitted timestamp", target.table())
                        })?,
                )
            };
            let mut rows = match target {
                RetentionTarget::ModelUsageObservations => statement.query(params![cutoff_ms]),
                _ => statement.query(params![request.cutoff]),
            }
            .map_err(|error| format!("Read {} archive rows: {error}", target.table()))?;

            let mut target_written = 0_i64;
            while let Some(row) = rows
                .next()
                .map_err(|error| format!("Read {} archive row: {error}", target.table()))?
            {
                let classification = match timestamp_index {
                    None => "delete_candidate",
                    Some(timestamp_index) => {
                        let timestamp = match row.get_ref(timestamp_index).map_err(|error| {
                            format!("Read {} archive timestamp: {error}", target.table())
                        })? {
                            ValueRef::Text(value) => {
                                std::str::from_utf8(value).map_err(|error| {
                                    format!("Decode {} archive timestamp: {error}", target.table())
                                })?
                            }
                            _ => {
                                return Err(format!(
                                    "{} archive timestamp was not stored as text",
                                    target.table()
                                ));
                            }
                        };
                        if is_conforming_timestamp(timestamp) {
                            "delete_candidate"
                        } else {
                            "nonconforming_retained"
                        }
                    }
                };

                let mut archived_row = Map::with_capacity(column_names.len());
                for (index, name) in column_names.iter().enumerate() {
                    let value = sqlite_value_to_json(row.get_ref(index).map_err(|error| {
                        format!("Read {} archive column {name}: {error}", target.table())
                    })?)?;
                    archived_row.insert(name.clone(), value);
                }
                serde_json::to_writer(
                    &mut writer,
                    &json!({
                        "record_type": "row",
                        "table": target.table(),
                        "classification": classification,
                        "row": Value::Object(archived_row),
                    }),
                )
                .map_err(|error| {
                    format!(
                        "Serialize {} retention archive row: {error}",
                        target.table()
                    )
                })?;
                writer.write_all(b"\n").map_err(|error| {
                    format!("Write {} retention archive row: {error}", target.table())
                })?;

                target_written += 1;
                written_total += 1;
                if let Some(sink) = progress
                    && (written_total % 1_000 == 0 || written_total == expected_total)
                {
                    let pct = if expected_total <= 0 {
                        100
                    } else {
                        ((written_total * 100) / expected_total).clamp(0, 100) as u8
                    };
                    sink(pct);
                }
            }
            written = target.with_count(&written, target_written);
        }

        if written != expected {
            return Err(format!(
                "Retention archive row count changed during the quiesced run: expected {} tool actions, {} session events, and {} model observations; wrote {}, {}, and {}",
                expected.tool_actions,
                expected.session_events,
                expected.model_usage_observations,
                written.tool_actions,
                written.session_events,
                written.model_usage_observations,
            ));
        }
        writer
            .flush()
            .map_err(|error| format!("Flush retention archive: {error}"))?;
    }
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("Sync retention archive: {error}"))?;
    temporary
        .persist_noclobber(&final_path)
        .map_err(|error| format!("Publish retention archive: {}", error.error))?;

    if let Some(sink) = progress {
        sink(100);
    }
    Ok(RetentionArchiveReport {
        path: final_path,
        rows: expected,
    })
}

fn sqlite_value_to_json(value: ValueRef<'_>) -> Result<Value, String> {
    match value {
        ValueRef::Null => Ok(Value::Null),
        ValueRef::Integer(value) => Ok(Value::from(value)),
        ValueRef::Real(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| "Retention archive encountered a non-finite number".to_string()),
        ValueRef::Text(value) => std::str::from_utf8(value)
            .map(|value| Value::String(value.to_string()))
            .map_err(|error| format!("Decode retention archive text: {error}")),
        ValueRef::Blob(value) => Ok(json!({
            "encoding": "hex",
            "data": hex::encode(value),
        })),
    }
}

fn report_scan_progress(controls: &RetentionDeleteControls<'_>, pct: u8) {
    if let Some(sink) = controls.scan_progress.as_ref() {
        sink(pct.min(100));
    }
}

/// Mutable state the Counting heartbeat carries between invocations.
struct ScanHeartbeat {
    sink: Arc<dyn Fn(u8) + Send + Sync>,
    last_emit: Instant,
    advanced: u8,
    base: u8,
}

/// Install the wall-clock heartbeat for one table's half of the scan.
///
/// The handler must be `'static`, so the sink is an [`Arc`] rather than a
/// borrow of the caller's. rusqlite additionally requires the closure to be
/// `RefUnwindSafe`, which a boxed `Fn` is not; the state is therefore wrapped
/// in [`AssertUnwindSafe`], which is sound here because the handler owns no
/// invariant a panic could tear — it holds a percentage and an instant, and
/// the connection is dropped on the unwind path regardless.
///
/// The handler never interrupts the statement — returning `true` here would
/// abort the scan — and it never climbs past [`SCAN_HEARTBEAT_CEILING`],
/// because a liveness signal that reaches 100% has started claiming a
/// completion it cannot see.
fn install_scan_heartbeat(conn: &Connection, controls: &RetentionDeleteControls<'_>, base: u8) {
    let Some(sink) = controls.scan_progress.as_ref().map(Arc::clone) else {
        return;
    };
    let mut state = AssertUnwindSafe(ScanHeartbeat {
        sink,
        last_emit: Instant::now(),
        advanced: 0,
        base,
    });
    conn.progress_handler(
        SCAN_HEARTBEAT_VM_OPS,
        Some(move || {
            // Name the wrapper as a whole first: edition-2021 disjoint capture
            // would otherwise capture `state.0` directly and drop the
            // unwind-safety assertion the closure's bound depends on.
            let wrapper = &mut state;
            let state = &mut wrapper.0;
            let now = Instant::now();
            if now.duration_since(state.last_emit) >= SCAN_HEARTBEAT_INTERVAL {
                state.last_emit = now;
                state.advanced = state.advanced.saturating_add(1).min(SCAN_HEARTBEAT_CEILING);
                (state.sink)(state.base.saturating_add(state.advanced).min(100));
            }
            false
        }),
    );
}

fn clear_scan_heartbeat(conn: &Connection) {
    conn.progress_handler(0, None::<fn() -> bool>);
}

/// Check that the delete phase has room to run before it removes anything.
///
/// This is **not** the VACUUM preflight and does not subsume it: VACUUM needs
/// twice the whole file, while the delete phase needs one chunk of WAL plus
/// the doomed-rowid temp tables. A database can comfortably pass this and fail
/// that, which is the legitimate "rows removed, bytes not yet reclaimed"
/// outcome; the reverse means no row may be removed at all.
pub fn preflight_delete_phase(
    db_path: &Path,
    doomed_rows: u64,
    controls: &RetentionDeleteControls<'_>,
) -> Result<RetentionDeleteBudget, RetentionPreflightFailure> {
    let budget = RetentionDeleteBudget::estimate(doomed_rows, controls.chunk_rows());
    let directory = db_path.parent().unwrap_or_else(|| Path::new("."));
    let available_bytes = controls
        .probe_free_space(directory)
        .map_err(|reason| RetentionPreflightFailure::Unreadable { reason })?;
    if available_bytes < budget.required_bytes {
        return Err(RetentionPreflightFailure::InsufficientSpace {
            required_bytes: budget.required_bytes,
            available_bytes,
        });
    }
    Ok(budget)
}

/// Outcome of draining both temp tables.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DrainOutcome {
    Completed,
    Partial { error_reason: String },
    Interrupted,
}

/// State threaded through the chunk loop.
///
/// Every transition produces a new value rather than editing one in place, so
/// a caller can never observe a half-updated tally.
#[derive(Clone, Copy, Debug, Default)]
struct DrainState {
    deleted: RetentionTableCounts,
    chunks_committed: u32,
}

impl DrainState {
    fn with_chunk(self, target: RetentionTarget, rows: i64) -> Self {
        Self {
            deleted: target.with_count(&self.deleted, target.counts_field(&self.deleted) + rows),
            chunks_committed: self.chunks_committed + 1,
        }
    }

    fn total_deleted(&self) -> i64 {
        self.deleted.tool_actions + self.deleted.session_events
    }
}

/// Drain one target table in chunks bounded by a single scalar rowid.
///
/// `on_first_chunk` runs once, before the very first chunk transaction of the
/// whole run opens. That is where the watermark advance lives, and it cannot
/// move inside the transaction: the watermark rides the primary connection,
/// WAL permits exactly one writer, and a primary-connection write issued while
/// this connection holds an `IMMEDIATE` transaction would deadlock the run
/// against itself until `busy_timeout` expired. Advancing just before the
/// commit rather than just after also closes the only window in which rows
/// could be gone while the watermark still permitted their reinsertion.
#[allow(clippy::too_many_arguments)]
fn drain_target(
    conn: &mut Connection,
    db_path: &Path,
    target: RetentionTarget,
    total_doomed: i64,
    budget: RetentionDeleteBudget,
    controls: &RetentionDeleteControls<'_>,
    state: DrainState,
    on_first_chunk: &mut dyn FnMut() -> Result<(), RetentionDeleteError>,
) -> Result<(DrainState, DrainOutcome), RetentionDeleteError> {
    let boundary_sql = format!(
        "SELECT max(rid) FROM (SELECT rid FROM {} ORDER BY rid LIMIT ?1)",
        target.doomed_table()
    );
    let target_delete_sql = format!(
        "DELETE FROM {} WHERE rowid <= ?1 AND rowid IN (SELECT rid FROM {})",
        target.table(),
        target.doomed_table()
    );
    // Roll each committed chunk into the durable daily view before deleting
    // its detail.  Keeping this statement in the same transaction gives the
    // retention boundary one atomic meaning: either both the aggregate and
    // its contributing raw rows survive, or neither change is visible.
    let target_aggregate_sql = match target {
        RetentionTarget::ToolActions => Some(format!(
            "INSERT INTO retention_daily_aggregates (
                 provider, source_key, session_id, day, agent_id, file_path,
                 tool_action_count, session_event_count, code_change_count,
                 lines_added, lines_removed
             )
             SELECT provider, source_key, session_id, substr(timestamp, 1, 10),
                    COALESCE(agent_id, ''), COALESCE(file_path, ''),
                    COUNT(*), 0, SUM(CASE WHEN category = 'code_change' THEN 1 ELSE 0 END),
                    COALESCE(SUM(CASE WHEN category = 'code_change'
                                      THEN COALESCE(lines_added, 0) ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN category = 'code_change'
                                      THEN COALESCE(lines_removed, 0) ELSE 0 END), 0)
             FROM tool_actions
             WHERE rowid <= ?1 AND rowid IN (SELECT rid FROM {})
             GROUP BY provider, source_key, session_id, substr(timestamp, 1, 10),
                      COALESCE(agent_id, ''), COALESCE(file_path, '')
             ON CONFLICT(provider, source_key, session_id, day, agent_id, file_path)
             DO UPDATE SET
                 tool_action_count = tool_action_count + excluded.tool_action_count,
                 code_change_count = code_change_count + excluded.code_change_count,
                 lines_added = lines_added + excluded.lines_added,
                 lines_removed = lines_removed + excluded.lines_removed",
            target.doomed_table()
        )),
        RetentionTarget::SessionEvents => Some(format!(
            "INSERT INTO retention_daily_aggregates (
                 provider, source_key, session_id, day, agent_id, file_path,
                 tool_action_count, session_event_count, code_change_count,
                 lines_added, lines_removed
             )
             SELECT provider, source_key, session_id, substr(timestamp, 1, 10),
                    COALESCE(agent_id, ''), '', 0, COUNT(*), 0, 0, 0
             FROM session_events
             WHERE rowid <= ?1 AND rowid IN (SELECT rid FROM {})
             GROUP BY provider, source_key, session_id, substr(timestamp, 1, 10),
                      COALESCE(agent_id, '')
             ON CONFLICT(provider, source_key, session_id, day, agent_id, file_path)
             DO UPDATE SET session_event_count =
                 session_event_count + excluded.session_event_count",
            target.doomed_table()
        )),
        // Model evidence has no transcript aggregate representation. It is
        // retained only as normalized detail, so its chunk deletes must not
        // fabricate a row in retention_daily_aggregates.
        RetentionTarget::ModelUsageObservations => None,
    };
    let bookkeeping_delete_sql = format!("DELETE FROM {} WHERE rid <= ?1", target.doomed_table());
    let remaining_sql = format!("SELECT COUNT(*) FROM {}", target.doomed_table());

    let chunk_rows = controls.chunk_rows();
    let recheck_interval = controls.recheck_interval();
    let mut state = state;

    loop {
        // Disk can be consumed by another process while the run is in flight,
        // so a preflight that passed at chunk 0 says nothing about chunk 400.
        if state.chunks_committed > 0 && state.chunks_committed.is_multiple_of(recheck_interval) {
            let directory = db_path.parent().unwrap_or_else(|| Path::new("."));
            match controls.probe_free_space(directory) {
                Ok(available) if available < budget.required_bytes => {
                    return Ok((
                        state,
                        DrainOutcome::Partial {
                            error_reason: RetentionPreflightFailure::InsufficientSpace {
                                required_bytes: budget.required_bytes,
                                available_bytes: available,
                            }
                            .to_string(),
                        },
                    ));
                }
                Ok(_) => {}
                Err(reason) => {
                    return Ok((
                        state,
                        DrainOutcome::Partial {
                            error_reason: RetentionPreflightFailure::Unreadable { reason }
                                .to_string(),
                        },
                    ));
                }
            }
        }

        if state.chunks_committed == 0 {
            on_first_chunk()?;
        }

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let boundary: Option<i64> =
            tx.query_row(&boundary_sql, params![chunk_rows as i64], |row| row.get(0))?;
        let Some(boundary) = boundary else {
            tx.rollback()?;
            return Ok((state, DrainOutcome::Completed));
        };
        if let Some(target_aggregate_sql) = &target_aggregate_sql {
            tx.execute(target_aggregate_sql, params![boundary])?;
        }
        let deleted = tx.execute(&target_delete_sql, params![boundary])?;
        let cleared = tx.execute(&bookkeeping_delete_sql, params![boundary])?;
        tx.commit()?;

        // Bounded by one chunk rather than by the run: under the quiesce lease
        // there is no competing writer to hold the WAL open.
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;

        let remaining: i64 = conn.query_row(&remaining_sql, [], |row| row.get(0))?;
        if cleared == 0 {
            return Err(RetentionDeleteError::ChunkStalled {
                table: target.table(),
                remaining: remaining.max(0) as u64,
            });
        }

        state = state.with_chunk(target, deleted as i64);

        if let Some(sink) = controls.delete_progress {
            let pct = if total_doomed <= 0 {
                100
            } else {
                ((state.total_deleted() * 100) / total_doomed).clamp(0, 100) as u8
            };
            sink(pct);
        }

        if let Some(hook) = controls.after_chunk {
            let report = RetentionChunkReport {
                target,
                chunk_index: state.chunks_committed,
                rows_deleted: deleted as u64,
                rows_remaining: remaining.max(0) as u64,
            };
            if hook(&report) == RetentionChunkControl::Interrupt {
                return Ok((state, DrainOutcome::Interrupted));
            }
        }
    }
}

/// Scan, preflight, delete in chunks, and persist the audit record.
///
/// The whole delete phase, on a connection this function opens and drops. The
/// caller supplies the quiesce lease, the confirmed cutoff and the before
/// bytes, and takes the returned report to the VACUUM preflight — which it may
/// only reach after this function has returned, because the maintenance
/// connection and its temp tables must be gone first.
///
/// The audit record is written on **every** path — completed, partial, skipped
/// — because a run the user cannot account for afterwards is the failure this
/// record exists to prevent. Its `bytes_after` equals `bytes_before` here,
/// which is not a placeholder but the truth: deletes free no filesystem bytes,
/// only the VACUUM that may follow does.
pub fn run_retention_delete_phase(
    storage: &Storage,
    request: &RetentionDeleteRequest,
    controls: &RetentionDeleteControls<'_>,
) -> Result<RetentionDeletePhaseReport, RetentionDeleteError> {
    if !is_conforming_timestamp(&request.cutoff) {
        return Err(RetentionDeleteError::MalformedCutoff {
            cutoff: request.cutoff.clone(),
        });
    }

    let db_path = storage.database_path().to_path_buf();
    let mut conn = open_maintenance_connection(&db_path)?;
    let scan = scan_doomed_rows(&conn, &request.cutoff, controls)?;

    if let Some(reason) = scan.skip_reason() {
        return finish(
            storage,
            request,
            RetentionRunStatus::Skipped,
            Some(reason.to_string()),
            None,
            &scan,
            RetentionTableCounts::default(),
            None,
            false,
            None,
        );
    }

    let archive = if let Some(directory) = controls.archive_directory {
        match write_retention_archive(&conn, directory, request, &scan, controls.archive_progress) {
            Ok(report) => Some(report),
            Err(reason) => {
                return finish(
                    storage,
                    request,
                    RetentionRunStatus::Skipped,
                    Some(format!(
                        "Could not archive the previewed rows; nothing was deleted: {reason}"
                    )),
                    None,
                    &scan,
                    RetentionTableCounts::default(),
                    None,
                    false,
                    None,
                );
            }
        }
    } else {
        None
    };

    let budget = match preflight_delete_phase(&db_path, scan.total_doomed().max(0) as u64, controls)
    {
        Ok(budget) => budget,
        Err(failure) => {
            // Nothing was deleted, so the watermark must not move: advancing
            // it here would suppress inserts the user never consented to lose.
            return finish(
                storage,
                request,
                RetentionRunStatus::Skipped,
                Some(failure.to_string()),
                None,
                &scan,
                RetentionTableCounts::default(),
                None,
                false,
                archive,
            );
        }
    };

    let mut watermark_advanced = false;
    let outcome = {
        let advanced = &mut watermark_advanced;
        let cutoff = request.cutoff.clone();
        // Idempotent: the first target can finish without committing a chunk
        // (nothing doomed in that table), in which case the second target's
        // first chunk is the run's first chunk and asks again.
        let mut on_first_chunk = move || -> Result<(), RetentionDeleteError> {
            if *advanced {
                return Ok(());
            }
            storage
                .advance_retention_watermark(&cutoff)
                .map_err(|reason| RetentionDeleteError::WatermarkAdvance { reason })?;
            *advanced = true;
            Ok(())
        };

        let mut state = DrainState::default();
        let mut outcome = DrainOutcome::Completed;
        for target in RetentionTarget::ALL {
            let (next, target_outcome) = drain_target(
                &mut conn,
                &db_path,
                target,
                scan.total_doomed(),
                budget,
                controls,
                state,
                &mut on_first_chunk,
            )?;
            state = next;
            if target_outcome != DrainOutcome::Completed {
                outcome = target_outcome;
                break;
            }
        }
        (state, outcome)
    };
    let (state, drain) = outcome;

    // The maintenance connection owns both temp tables and must be gone before
    // the caller can invoke VACUUM.
    drop(conn);

    let (status, error_reason) = match drain {
        DrainOutcome::Completed => (RetentionRunStatus::Completed, None),
        DrainOutcome::Partial { error_reason } => (RetentionRunStatus::Partial, Some(error_reason)),
        // A run stopped between chunks reports what actually committed, with
        // the interruption named, exactly as a mid-run failure does.
        DrainOutcome::Interrupted => (
            RetentionRunStatus::Partial,
            Some("The retention run stopped between chunks".to_string()),
        ),
    };

    finish(
        storage,
        request,
        status,
        None,
        error_reason,
        &scan,
        state.deleted,
        Some(budget),
        watermark_advanced,
        archive,
    )
}

/// Persist the audit record and assemble the report, on every path.
#[allow(clippy::too_many_arguments)]
fn finish(
    storage: &Storage,
    request: &RetentionDeleteRequest,
    status: RetentionRunStatus,
    reason: Option<String>,
    error_reason: Option<String>,
    scan: &RetentionScanReport,
    deleted: RetentionTableCounts,
    budget: Option<RetentionDeleteBudget>,
    watermark_advanced: bool,
    archive: Option<RetentionArchiveReport>,
) -> Result<RetentionDeletePhaseReport, RetentionDeleteError> {
    let mut audit = RetentionAuditRecord::new(status, request.ran_at)
        .with_window(request.window_days, request.cutoff.clone())
        .with_deleted(deleted)
        .with_skipped_nonconforming(scan.nonconforming)
        // Deletes reclaim nothing; only the VACUUM the caller may run next
        // changes the file's size, and it rewrites this record when it does.
        .with_bytes(request.bytes_before, request.bytes_before);
    if let Some(reason) = reason.clone() {
        audit = audit.with_reason(reason);
    }
    if let Some(error_reason) = error_reason.clone() {
        audit = audit.with_error_reason(error_reason);
    }

    storage
        .write_retention_audit_record(&audit)
        .map_err(|reason| RetentionDeleteError::AuditWrite { reason })?;

    Ok(RetentionDeletePhaseReport {
        status,
        reason,
        error_reason,
        doomed: scan.doomed,
        deleted,
        nonconforming: scan.nonconforming,
        budget,
        audit,
        watermark_advanced,
        archive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention_fixture::{
        RetentionFixture, RetentionFixtureSpec, RetentionRowKind, RetentionTable,
        build_retention_fixture, count_rows, count_rows_before,
    };
    use serial_test::serial;
    use std::cell::{Cell, RefCell};
    use std::sync::Mutex as StdMutex;

    /// Buckets the cutoff retains. Buckets 3..6 are doomed.
    const MONTHS_RETAINED: u32 = 3;

    /// Deliberately smaller than either table's doomed set, so every test that
    /// deletes exercises the chunk loop rather than a single sweep.
    const TEST_CHUNK_ROWS: u64 = 5;

    fn fixture_spec() -> RetentionFixtureSpec {
        RetentionFixtureSpec {
            // A fixed anchor keeps every derived boundary stable regardless of
            // when the suite runs.
            anchor: DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
                .expect("parse anchor")
                .with_timezone(&Utc),
            months: 6,
            owned_rows_per_month: 12,
            live_rows_per_month: 4,
            sources: 3,
        }
    }

    fn target_table(target: RetentionTarget) -> RetentionTable {
        match target {
            RetentionTarget::ToolActions => RetentionTable::ToolActions,
            RetentionTarget::SessionEvents => RetentionTable::SessionEvents,
            RetentionTarget::ModelUsageObservations => {
                unreachable!("the transcript fixture has no model-observation lane")
            }
        }
    }

    fn request(fixture: &RetentionFixture) -> RetentionDeleteRequest {
        RetentionDeleteRequest {
            cutoff: fixture.plan().boundary_timestamp(MONTHS_RETAINED),
            window_days: 90,
            bytes_before: std::fs::metadata(fixture.db_path())
                .expect("stat fixture database")
                .len(),
            ran_at: fixture.plan().anchor(),
        }
    }

    fn doomed_rows(fixture: &RetentionFixture, target: RetentionTarget) -> i64 {
        fixture.plan().rows_before_boundary(
            MONTHS_RETAINED,
            target_table(target),
            RetentionRowKind::OwnedConforming,
        ) as i64
    }

    fn chunked(chunk_rows: u64) -> RetentionDeleteControls<'static> {
        RetentionDeleteControls {
            chunk_rows,
            ..RetentionDeleteControls::default()
        }
    }

    /// Plant one owned, conforming row whose timestamp is *exactly* the cutoff
    /// in both target tables. The fixture never lands a row on a boundary, so
    /// without this the strict `<` predicate is untested at its own edge.
    fn plant_boundary_rows(fixture: &RetentionFixture, cutoff: &str) {
        let conn = fixture.open_connection().expect("open fixture connection");
        conn.execute(
            "INSERT INTO tool_actions
                 (provider, source_key, action_key, message_id, session_id, chain_id,
                  tool_name, category, summary, timestamp)
             VALUES ('claude', 'retention-fixture/source-0000.jsonl', 'ta-boundary',
                     'ta-boundary', 'session-0000', 'chain-0000', 'Read', 'tool_detail',
                     'boundary', ?1)",
            params![cutoff],
        )
        .expect("plant tool_actions boundary row");
        conn.execute(
            "INSERT INTO session_events
                 (provider, source_key, event_key, session_id, chain_id, timestamp, kind)
             VALUES ('claude', 'retention-fixture/source-0000.jsonl', 'se-boundary',
                     'session-0000', 'chain-0000', ?1, 'user_prompt')",
            params![cutoff],
        )
        .expect("plant session_events boundary row");
    }

    fn boundary_rows_present(fixture: &RetentionFixture) -> (i64, i64) {
        let conn = fixture.open_connection().expect("open fixture connection");
        let tool_actions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tool_actions WHERE action_key = 'ta-boundary'",
                [],
                |row| row.get(0),
            )
            .expect("count planted tool_actions boundary row");
        let session_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE event_key = 'se-boundary'",
                [],
                |row| row.get(0),
            )
            .expect("count planted session_events boundary row");
        (tool_actions, session_events)
    }

    fn count(fixture: &RetentionFixture, table: RetentionTable, kind: RetentionRowKind) -> u64 {
        let conn = fixture.open_connection().expect("open fixture connection");
        count_rows(&conn, table, kind).expect("count rows")
    }

    fn count_before(
        fixture: &RetentionFixture,
        table: RetentionTable,
        kind: RetentionRowKind,
        cutoff: &str,
    ) -> u64 {
        let conn = fixture.open_connection().expect("open fixture connection");
        count_rows_before(&conn, table, kind, cutoff).expect("count rows before cutoff")
    }

    // @lat: [[backend#Backend#Database#Retention delete engine#Retention Delete Engine Test Specs#Chunk Correctness And Idempotency]]
    #[test]
    #[serial]
    fn chunked_delete_is_exact_and_idempotent() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        let request = request(&fixture);
        plant_boundary_rows(&fixture, &request.cutoff);

        let expected_tool_actions = doomed_rows(&fixture, RetentionTarget::ToolActions);
        let expected_session_events = doomed_rows(&fixture, RetentionTarget::SessionEvents);
        assert!(
            expected_tool_actions > TEST_CHUNK_ROWS as i64,
            "the chunk size must be smaller than the doomed set or nothing is chunked"
        );

        let chunks: RefCell<Vec<RetentionChunkReport>> = RefCell::new(Vec::new());
        let record = |report: &RetentionChunkReport| {
            chunks.borrow_mut().push(*report);
            RetentionChunkControl::Continue
        };
        let scan_pcts: Arc<StdMutex<Vec<u8>>> = Arc::new(StdMutex::new(Vec::new()));
        let sink_pcts = Arc::clone(&scan_pcts);
        let controls = RetentionDeleteControls {
            chunk_rows: TEST_CHUNK_ROWS,
            after_chunk: Some(&record),
            scan_progress: Some(Arc::new(move |pct| {
                sink_pcts.lock().expect("lock scan pcts").push(pct)
            })),
            ..RetentionDeleteControls::default()
        };

        let report =
            run_retention_delete_phase(&storage, &request, &controls).expect("run delete phase");

        assert_eq!(RetentionRunStatus::Completed, report.status);
        assert_eq!(None, report.reason);
        assert_eq!(None, report.error_reason);
        assert_eq!(expected_tool_actions, report.deleted.tool_actions);
        assert_eq!(expected_session_events, report.deleted.session_events);
        assert_eq!(report.doomed, report.deleted);
        assert!(report.watermark_advanced);

        // The temp table drains in lockstep: after every chunk the rowids
        // still queued equal the doomed set minus everything committed so far.
        let recorded = chunks.borrow();
        assert!(recorded.len() >= 2, "the run must have committed chunks");
        for target in RetentionTarget::TRANSCRIPT_TARGETS {
            let mut cumulative = 0_u64;
            let expected = target.counts_field(&report.doomed) as u64;
            for chunk in recorded.iter().filter(|chunk| chunk.target == target) {
                cumulative += chunk.rows_deleted;
                assert_eq!(
                    expected - cumulative,
                    chunk.rows_remaining,
                    "{} temp table diverged from its target at chunk {}",
                    target.table(),
                    chunk.chunk_index
                );
            }
            assert_eq!(expected, cumulative);
        }
        drop(recorded);

        // The heartbeat installs, fires all three per-table completion nudges,
        // and uninstalls without disturbing the scan.
        let pcts = scan_pcts.lock().expect("lock scan pcts");
        assert!(
            pcts.contains(&33) && pcts.contains(&66) && pcts.contains(&100),
            "{pcts:?}"
        );
        drop(pcts);

        // Strict `<`: a row whose timestamp equals the cutoff is retained.
        assert_eq!((1, 1), boundary_rows_present(&fixture));

        for target in RetentionTarget::TRANSCRIPT_TARGETS {
            assert_eq!(
                0,
                count_before(
                    &fixture,
                    target_table(target),
                    RetentionRowKind::OwnedConforming,
                    &request.cutoff
                )
            );
        }
        // Sibling tables keep full history.
        for table in [
            RetentionTable::ResponseTimes,
            RetentionTable::SkillUsages,
            RetentionTable::HookInvocations,
        ] {
            assert_eq!(
                fixture
                    .plan()
                    .total_rows(table, RetentionRowKind::OwnedConforming),
                count(&fixture, table, RetentionRowKind::OwnedConforming),
                "{} must be untouched",
                table.as_str()
            );
        }

        // An immediate re-run has nothing to do and says so.
        let rerun = run_retention_delete_phase(&storage, &request, &chunked(TEST_CHUNK_ROWS))
            .expect("re-run delete phase");
        assert_eq!(RetentionRunStatus::Skipped, rerun.status);
        assert_eq!(
            Some(RETENTION_NOTHING_OLDER_REASON.to_string()),
            rerun.reason
        );
        assert_eq!(RetentionTableCounts::default(), rerun.deleted);
        assert!(!rerun.watermark_advanced);

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Database#Retention delete engine#Retention Delete Engine Test Specs#Interrupted Run Stays Consistent]]
    #[test]
    #[serial]
    fn an_interrupted_run_leaves_a_consistent_database() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        let request = request(&fixture);
        let total_doomed = doomed_rows(&fixture, RetentionTarget::ToolActions)
            + doomed_rows(&fixture, RetentionTarget::SessionEvents);

        let stop_after = |report: &RetentionChunkReport| {
            if report.chunk_index >= 2 {
                RetentionChunkControl::Interrupt
            } else {
                RetentionChunkControl::Continue
            }
        };
        let controls = RetentionDeleteControls {
            chunk_rows: TEST_CHUNK_ROWS,
            after_chunk: Some(&stop_after),
            ..RetentionDeleteControls::default()
        };

        let interrupted =
            run_retention_delete_phase(&storage, &request, &controls).expect("run delete phase");
        let committed = interrupted.deleted.tool_actions + interrupted.deleted.session_events;
        assert_eq!(2 * TEST_CHUNK_ROWS as i64, committed);
        assert_eq!(RetentionRunStatus::Partial, interrupted.status);

        // Committed chunks stayed committed, and nothing partial was written:
        // the surviving pre-cutoff rows are exactly the ones no chunk reached.
        assert_eq!(
            (total_doomed - committed) as u64,
            count_before(
                &fixture,
                RetentionTable::ToolActions,
                RetentionRowKind::OwnedConforming,
                &request.cutoff
            ) + count_before(
                &fixture,
                RetentionTable::SessionEvents,
                RetentionRowKind::OwnedConforming,
                &request.cutoff
            )
        );

        // The next run needs no special handling: it rescans and finishes.
        let resumed = run_retention_delete_phase(&storage, &request, &chunked(TEST_CHUNK_ROWS))
            .expect("resume delete phase");
        assert_eq!(RetentionRunStatus::Completed, resumed.status);
        assert_eq!(
            total_doomed - committed,
            resumed.deleted.tool_actions + resumed.deleted.session_events
        );
        for target in RetentionTarget::TRANSCRIPT_TARGETS {
            assert_eq!(
                0,
                count_before(
                    &fixture,
                    target_table(target),
                    RetentionRowKind::OwnedConforming,
                    &request.cutoff
                )
            );
        }

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Database#Retention delete engine#Retention Delete Engine Test Specs#Watermark Advances At First Chunk]]
    #[test]
    #[serial]
    fn the_watermark_reaches_the_cutoff_at_the_first_chunk() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        let request = request(&fixture);

        let stop_after_first = |report: &RetentionChunkReport| {
            if report.chunk_index >= 1 {
                RetentionChunkControl::Interrupt
            } else {
                RetentionChunkControl::Continue
            }
        };
        let controls = RetentionDeleteControls {
            chunk_rows: TEST_CHUNK_ROWS,
            after_chunk: Some(&stop_after_first),
            ..RetentionDeleteControls::default()
        };

        let report =
            run_retention_delete_phase(&storage, &request, &controls).expect("run delete phase");
        assert_eq!(TEST_CHUNK_ROWS as i64, report.deleted.tool_actions);

        // Killed after chunk 1, the watermark is already at the run's cutoff —
        // not at the end of the run, and emphatically not after VACUUM.
        let watermark = storage
            .read_retention_watermark()
            .expect("read watermark")
            .expect("watermark is set");
        assert_eq!(request.cutoff, watermark);

        // Every row those chunks removed was strictly older than the stored
        // watermark, so an insert filter honouring it cannot resurrect them.
        assert_eq!(
            (doomed_rows(&fixture, RetentionTarget::ToolActions) - TEST_CHUNK_ROWS as i64) as u64,
            count_before(
                &fixture,
                RetentionTable::ToolActions,
                RetentionRowKind::OwnedConforming,
                &watermark
            )
        );

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Database#Retention delete engine#Retention Delete Engine Test Specs#Preflight Skip Leaves The Watermark]]
    #[test]
    #[serial]
    fn a_failing_delete_preflight_removes_nothing_and_leaves_the_watermark() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        let request = request(&fixture);

        let earlier = fixture.plan().boundary_timestamp(MONTHS_RETAINED + 1);
        let before = storage
            .advance_retention_watermark(&earlier)
            .expect("seed watermark");

        let starved = |_: &Path| Ok(0_u64);
        let controls = RetentionDeleteControls {
            chunk_rows: TEST_CHUNK_ROWS,
            free_space: Some(&starved),
            ..RetentionDeleteControls::default()
        };

        let report =
            run_retention_delete_phase(&storage, &request, &controls).expect("run delete phase");

        assert_eq!(RetentionRunStatus::Skipped, report.status);
        assert!(
            report
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("Insufficient free disk space")),
            "{:?}",
            report.reason
        );
        assert_eq!(RetentionTableCounts::default(), report.deleted);
        assert!(!report.watermark_advanced);
        assert_eq!(RetentionRunStatus::Skipped, report.audit.status);

        // Byte-identical: advancing here would suppress inserts the user never
        // consented to lose, because nothing was deleted.
        assert_eq!(
            Some(before),
            storage.read_retention_watermark().expect("read watermark")
        );
        for target in RetentionTarget::TRANSCRIPT_TARGETS {
            assert_eq!(
                target.counts_field(&report.doomed) as u64,
                count_before(
                    &fixture,
                    target_table(target),
                    RetentionRowKind::OwnedConforming,
                    &request.cutoff
                ),
                "no row may be removed when the delete budget fails"
            );
        }

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Database#Retention delete engine#Retention Delete Engine Test Specs#Mid Run Failure Reports Partial]]
    #[test]
    #[serial]
    fn a_mid_run_free_space_failure_stops_cleanly_as_partial() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        let request = request(&fixture);

        // The preflight sees a healthy disk; every later re-check sees none.
        let probes = Cell::new(0_u32);
        let exhausting = |_: &Path| {
            let seen = probes.get();
            probes.set(seen + 1);
            if seen == 0 { Ok(u64::MAX) } else { Ok(0) }
        };
        let recheck_every = 3_u32;
        let controls = RetentionDeleteControls {
            chunk_rows: TEST_CHUNK_ROWS,
            free_space_recheck_chunks: recheck_every,
            free_space: Some(&exhausting),
            ..RetentionDeleteControls::default()
        };

        let report =
            run_retention_delete_phase(&storage, &request, &controls).expect("run delete phase");

        assert_eq!(RetentionRunStatus::Partial, report.status);
        assert_eq!(None, report.reason, "reason stays the skip slot");
        assert!(
            report
                .error_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("Insufficient free disk space")),
            "{:?}",
            report.error_reason
        );
        let committed = (u64::from(recheck_every) * TEST_CHUNK_ROWS) as i64;
        assert_eq!(committed, report.deleted.tool_actions);
        assert_eq!(0, report.deleted.session_events);
        assert_eq!(
            (doomed_rows(&fixture, RetentionTarget::ToolActions) - committed) as u64,
            count_before(
                &fixture,
                RetentionTable::ToolActions,
                RetentionRowKind::OwnedConforming,
                &request.cutoff
            )
        );

        // Written on the error path, exactly like a success.
        let persisted = storage
            .read_retention_audit_record()
            .expect("read audit record")
            .expect("a partial run persists its record");
        assert_eq!(report.audit, persisted);
        assert_eq!(RetentionRunStatus::Partial, persisted.status);
        assert_eq!(committed, persisted.deleted.tool_actions);
        // Deletes reclaim nothing; only a VACUUM does, and a partial run does
        // not get one.
        assert_eq!(persisted.bytes_before, persisted.bytes_after);

        // The watermark, advanced at the first chunk, stays advanced.
        assert!(report.watermark_advanced);
        assert_eq!(
            Some(request.cutoff),
            storage.read_retention_watermark().expect("read watermark")
        );

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Database#Retention delete engine#Retention Delete Engine Test Specs#Non Conforming Rows Retained]]
    #[test]
    #[serial]
    fn non_conforming_rows_are_retained_and_reported() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        let request = request(&fixture);

        let before: Vec<u64> = RetentionTarget::TRANSCRIPT_TARGETS
            .iter()
            .map(|target| {
                count(
                    &fixture,
                    target_table(*target),
                    RetentionRowKind::OwnedNonConforming,
                )
            })
            .collect();
        assert!(before.iter().all(|count| *count > 0));

        let report = run_retention_delete_phase(&storage, &request, &chunked(TEST_CHUNK_ROWS))
            .expect("run delete phase");
        assert_eq!(RetentionRunStatus::Completed, report.status);

        for (index, target) in RetentionTarget::TRANSCRIPT_TARGETS.into_iter().enumerate() {
            let table = target_table(target);
            // Pre-cutoff `+00:00` rows survive …
            assert_eq!(
                before[index],
                count(&fixture, table, RetentionRowKind::OwnedNonConforming)
            );
            // … and are reported rather than silently ignored.
            let expected = fixture.plan().rows_before_boundary(
                MONTHS_RETAINED,
                table,
                RetentionRowKind::OwnedNonConforming,
            ) as i64;
            assert!(expected > 0);
            assert_eq!(expected, target.counts_field(&report.nonconforming));
            assert_eq!(
                expected,
                target.counts_field(&report.audit.skipped_nonconforming)
            );
        }

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Database#Retention delete engine#Retention Delete Engine Test Specs#Delete And Vacuum Budgets Are Distinct]]
    #[test]
    #[serial]
    fn the_delete_budget_is_distinct_from_the_vacuum_budget() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        let request = request(&fixture);

        let total_doomed = (doomed_rows(&fixture, RetentionTarget::ToolActions)
            + doomed_rows(&fixture, RetentionTarget::SessionEvents))
            as u64;
        let budget = RetentionDeleteBudget::estimate(total_doomed, TEST_CHUNK_ROWS);
        let vacuum_required = request.bytes_before.saturating_mul(2);
        assert!(
            budget.required_bytes < vacuum_required,
            "the delete budget ({}) must be satisfiable where the VACUUM budget ({}) is not, \
             or the two checks are not distinct",
            budget.required_bytes,
            vacuum_required
        );
        assert!(budget.temp_bytes > 0, "the TEMP term is not optional");

        // Exactly enough for the delete phase, nowhere near enough for VACUUM.
        let squeezed = budget.required_bytes;
        let tight = move |_: &Path| Ok(squeezed);
        let controls = RetentionDeleteControls {
            chunk_rows: TEST_CHUNK_ROWS,
            free_space: Some(&tight),
            ..RetentionDeleteControls::default()
        };

        let report =
            run_retention_delete_phase(&storage, &request, &controls).expect("run delete phase");
        assert_eq!(RetentionRunStatus::Completed, report.status);
        assert_eq!(report.doomed, report.deleted);
        assert_eq!(Some(budget), report.budget);
        // Rows removed, bytes not yet reclaimed — the legitimate outcome the
        // composite command reports as `compaction_status: "skipped"`.
        assert_eq!(report.audit.bytes_before, report.audit.bytes_after);

        drop(storage);
        drop(fixture);
    }

    // @lat: [[backend#Backend#Database#Retention delete engine#Retention Delete Engine Test Specs#Live Rows Are Never Doomed]]
    #[test]
    #[serial]
    fn live_rows_are_never_selected_by_the_scan() {
        let fixture = build_retention_fixture(&fixture_spec()).expect("build fixture");
        let storage = Storage::init().expect("open storage on fixture");
        let request = request(&fixture);

        for target in RetentionTarget::TRANSCRIPT_TARGETS {
            let table = target_table(target);
            let older = count_before(&fixture, table, RetentionRowKind::Live, &request.cutoff);
            assert!(older > 0, "live rows must straddle the cutoff");
            assert!(count(&fixture, table, RetentionRowKind::Live) > older);
        }

        let conn = open_maintenance_connection(storage.database_path())
            .expect("open maintenance connection");
        let scan = scan_doomed_rows(&conn, &request.cutoff, &RetentionDeleteControls::default())
            .expect("scan doomed rows");
        for target in RetentionTarget::TRANSCRIPT_TARGETS {
            assert_eq!(
                doomed_rows(&fixture, target),
                target.counts_field(&scan.doomed),
                "the scan must select owned conforming rows and nothing else"
            );
        }
        drop(conn);

        let report = run_retention_delete_phase(&storage, &request, &chunked(TEST_CHUNK_ROWS))
            .expect("run delete phase");
        assert_eq!(RetentionRunStatus::Completed, report.status);

        for target in RetentionTarget::TRANSCRIPT_TARGETS {
            let table = target_table(target);
            assert_eq!(
                fixture.plan().total_rows(table, RetentionRowKind::Live),
                count(&fixture, table, RetentionRowKind::Live),
                "{} lost a live row",
                table.as_str()
            );
        }

        drop(storage);
        drop(fixture);
    }

    #[test]
    fn a_non_conforming_cutoff_is_refused_before_anything_is_scanned() {
        let conn = Connection::open_in_memory().expect("open in-memory connection");
        assert!(matches!(
            scan_doomed_rows(
                &conn,
                "2026-04-02T00:00:00Z",
                &RetentionDeleteControls::default(),
            ),
            Err(RetentionDeleteError::MalformedCutoff { .. })
        ));
    }

    #[test]
    fn the_delete_budget_prices_both_terms() {
        // The spike's corpus, so these are the figures its published preflight
        // terms (19,718,352 WAL / 7,753,728 TEMP) are meant to reproduce. They
        // land a few hundred bytes low because the published *rates* are the
        // measured totals rounded to one decimal, which is exactly why the
        // safety multiplier sits on top of them.
        let budget = RetentionDeleteBudget::estimate(701_400, RETENTION_CHUNK_ROWS);
        assert_eq!(19_717_500, budget.wal_bytes);
        assert_eq!(7_750_471, budget.temp_bytes);
        assert_eq!(
            (budget.wal_bytes + budget.temp_bytes) * RETENTION_PREFLIGHT_SAFETY_MULTIPLIER,
            budget.required_bytes
        );
        // A chunk never budgets for more rows than exist to delete.
        assert_eq!(
            RetentionDeleteBudget::estimate(10, RETENTION_CHUNK_ROWS).wal_bytes,
            (10.0 * RETENTION_WAL_BYTES_PER_ROW).ceil() as u64
        );
    }
}
