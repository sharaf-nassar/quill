//! Shared chunk runner for resumable rollup backfills.
//!
//! Targets own only their fold and metadata SQL. The runner owns the safety
//! boundaries around those callbacks: disk preflight, one immediate
//! transaction under the ingest read permit, atomic bookmark persistence,
//! permit release between chunks, and a WAL checkpoint after every commit.

#![allow(dead_code)]

use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::retention_engine::available_disk_space;
use crate::with_ingest_write_permit;

/// Hard ceiling communicated to every target callback.
pub(crate) const ROLLUP_BACKFILL_CHUNK_TARGET: Duration = Duration::from_millis(250);

/// Default WAL estimate, rounded up from the measured retention precedent.
pub(crate) const ROLLUP_BACKFILL_WAL_BYTES_PER_ROW: u64 = 789;

/// Default free-space headroom over one estimated chunk of WAL.
pub(crate) const ROLLUP_BACKFILL_SPACE_MULTIPLIER: u64 = 2;

/// Durable state loaded before a run or resume.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RollupBackfillState {
    pub(crate) rows_done: u64,
    pub(crate) done_through: Option<i64>,
}

/// Time and row bounds a target must honor while folding one chunk.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RollupBackfillChunkBudget {
    pub(crate) max_rows: u64,
    pub(crate) deadline: Instant,
}

impl RollupBackfillChunkBudget {
    pub(crate) fn should_yield(self) -> bool {
        Instant::now() >= self.deadline
    }
}

/// Target result staged in the same transaction as its rollup writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RollupBackfillChunk {
    pub(crate) rows_processed: u64,
    pub(crate) done_through: Option<i64>,
    pub(crate) completed: bool,
}

/// Minimal target-specific operations used by the shared runner.
pub(crate) trait RollupBackfillTarget {
    fn name(&self) -> &'static str;

    fn load_state(&self, conn: &Connection) -> rusqlite::Result<RollupBackfillState>;

    fn count_total_rows(&self, conn: &Connection) -> rusqlite::Result<u64>;

    /// Prepare expensive source reads outside the immediate transaction and
    /// ingest permit. Targets must revalidate prepared identity in
    /// `fold_chunk` before applying it atomically with the bookmark.
    fn prepare_chunk(
        &mut self,
        _conn: &Connection,
        _state: RollupBackfillState,
        _max_rows: u64,
    ) -> rusqlite::Result<()> {
        Ok(())
    }

    fn fold_chunk(
        &mut self,
        tx: &Transaction<'_>,
        state: RollupBackfillState,
        budget: RollupBackfillChunkBudget,
    ) -> rusqlite::Result<RollupBackfillChunk>;

    /// Persist the returned bookmark and lifecycle status.
    ///
    /// The runner invokes this before committing the transaction that contains
    /// the target fold, so neither half can become durable on its own.
    fn persist_chunk(
        &self,
        tx: &Transaction<'_>,
        state: RollupBackfillState,
        completed: bool,
    ) -> rusqlite::Result<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RollupBackfillPhase {
    Preflight,
    Folding,
    Checkpointing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RollupBackfillProgress {
    pub(crate) target: String,
    pub(crate) phase: RollupBackfillPhase,
    pub(crate) rows_done: u64,
    pub(crate) rows_total: u64,
    pub(crate) done_through: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RollupBackfillTerminalError {
    DiskSpaceProbeFailed {
        reason: String,
    },
    InsufficientDiskSpace {
        required_bytes: u64,
        available_bytes: u64,
    },
    CheckpointBusy {
        log_frames: i64,
        checkpointed_frames: i64,
    },
    CheckpointFailed {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RollupBackfillTerminal {
    Completed,
    Interrupted,
    Error(RollupBackfillTerminalError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RollupBackfillReport {
    pub(crate) progress: RollupBackfillProgress,
    pub(crate) terminal: RollupBackfillTerminal,
}

#[derive(Debug)]
pub(crate) enum RollupBackfillError {
    InvalidChunkRows,
    StateLoad(rusqlite::Error),
    CountRows(rusqlite::Error),
    BeginChunk(rusqlite::Error),
    FoldChunk(rusqlite::Error),
    PersistChunk(rusqlite::Error),
    CommitChunk(rusqlite::Error),
    ChunkStalled {
        target: String,
        done_through: Option<i64>,
    },
    BookmarkDidNotAdvance {
        target: String,
        before: i64,
        after: i64,
    },
    RowsDoneOverflow,
}

impl std::fmt::Display for RollupBackfillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidChunkRows => write!(f, "Rollup backfill chunk size must be positive"),
            Self::StateLoad(error) => write!(f, "Load rollup backfill state: {error}"),
            Self::CountRows(error) => write!(f, "Count rollup backfill rows: {error}"),
            Self::BeginChunk(error) => write!(f, "Begin rollup backfill chunk: {error}"),
            Self::FoldChunk(error) => write!(f, "Fold rollup backfill chunk: {error}"),
            Self::PersistChunk(error) => write!(f, "Persist rollup backfill bookmark: {error}"),
            Self::CommitChunk(error) => write!(f, "Commit rollup backfill chunk: {error}"),
            Self::ChunkStalled {
                target,
                done_through,
            } => write!(
                f,
                "Rollup backfill target {target} made no progress at {done_through:?}"
            ),
            Self::BookmarkDidNotAdvance {
                target,
                before,
                after,
            } => write!(
                f,
                "Rollup backfill target {target} moved its bookmark from {before} to {after}"
            ),
            Self::RowsDoneOverflow => write!(f, "Rollup backfill row count overflowed"),
        }
    }
}

impl std::error::Error for RollupBackfillError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StateLoad(error)
            | Self::CountRows(error)
            | Self::BeginChunk(error)
            | Self::FoldChunk(error)
            | Self::PersistChunk(error)
            | Self::CommitChunk(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RollupCheckpointResult {
    Complete,
    Busy {
        log_frames: i64,
        checkpointed_frames: i64,
    },
}

pub(crate) type RollupFreeSpaceProbe<'a> = &'a dyn Fn(&Path) -> Result<u64, String>;
pub(crate) type RollupCheckpoint<'a> =
    &'a dyn Fn(&Connection) -> Result<RollupCheckpointResult, String>;
pub(crate) type RollupProgressSink<'a> = &'a dyn Fn(&RollupBackfillProgress);
pub(crate) type RollupChunkHook<'a> = &'a dyn Fn(&RollupBackfillProgress) -> RollupChunkControl;
#[cfg(test)]
pub(crate) type RollupPermitHook<'a> = &'a dyn Fn();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RollupChunkControl {
    Continue,
    Interrupt,
}

pub(crate) struct RollupBackfillControls<'a> {
    pub(crate) chunk_rows: u64,
    pub(crate) max_chunk_duration: Duration,
    pub(crate) wal_bytes_per_row: u64,
    pub(crate) free_space_multiplier: u64,
    pub(crate) free_space: Option<RollupFreeSpaceProbe<'a>>,
    pub(crate) checkpoint: Option<RollupCheckpoint<'a>>,
    pub(crate) progress: Option<RollupProgressSink<'a>>,
    pub(crate) after_chunk: Option<RollupChunkHook<'a>>,
    #[cfg(test)]
    pub(crate) before_chunk_permit: Option<RollupPermitHook<'a>>,
    #[cfg(test)]
    pub(crate) after_chunk_permit: Option<RollupPermitHook<'a>>,
    #[cfg(test)]
    pub(crate) before_checkpoint: Option<RollupProgressSink<'a>>,
}

impl Default for RollupBackfillControls<'_> {
    fn default() -> Self {
        Self {
            chunk_rows: 5_000,
            max_chunk_duration: ROLLUP_BACKFILL_CHUNK_TARGET,
            wal_bytes_per_row: ROLLUP_BACKFILL_WAL_BYTES_PER_ROW,
            free_space_multiplier: ROLLUP_BACKFILL_SPACE_MULTIPLIER,
            free_space: None,
            checkpoint: None,
            progress: None,
            after_chunk: None,
            #[cfg(test)]
            before_chunk_permit: None,
            #[cfg(test)]
            after_chunk_permit: None,
            #[cfg(test)]
            before_checkpoint: None,
        }
    }
}

impl RollupBackfillControls<'_> {
    fn required_free_bytes(&self) -> u64 {
        self.chunk_rows
            .saturating_mul(self.wal_bytes_per_row)
            .saturating_mul(self.free_space_multiplier)
    }

    fn probe_free_space(&self, directory: &Path) -> Result<u64, String> {
        match self.free_space {
            Some(probe) => probe(directory),
            None => available_disk_space(directory),
        }
    }

    fn checkpoint(&self, conn: &Connection) -> Result<RollupCheckpointResult, String> {
        match self.checkpoint {
            Some(checkpoint) => checkpoint(conn),
            None => checkpoint_truncate(conn),
        }
    }

    fn chunk_duration(&self) -> Duration {
        self.max_chunk_duration
            .max(Duration::from_millis(1))
            .min(ROLLUP_BACKFILL_CHUNK_TARGET)
    }
}

/// Run or resume one target until completion or a safe terminal boundary.
// @lat: [[backend#Backend#Database#Schema#Hourly Analytics Rollups#Chunked Rollup Backfill Framework]]
pub(crate) fn run_rollup_backfill<T: RollupBackfillTarget>(
    conn: &mut Connection,
    db_path: &Path,
    target: &mut T,
    controls: &RollupBackfillControls<'_>,
) -> Result<RollupBackfillReport, RollupBackfillError> {
    if controls.chunk_rows == 0 {
        return Err(RollupBackfillError::InvalidChunkRows);
    }

    let target_name = target.name().to_string();
    let total = target
        .count_total_rows(conn)
        .map_err(RollupBackfillError::CountRows)?;
    let mut state = target
        .load_state(conn)
        .map_err(RollupBackfillError::StateLoad)?;
    let directory = db_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let required_bytes = controls.required_free_bytes();

    loop {
        let mut progress = RollupBackfillProgress {
            target: target_name.clone(),
            phase: RollupBackfillPhase::Preflight,
            rows_done: state.rows_done,
            rows_total: total,
            done_through: state.done_through,
        };
        let available_bytes = match controls.probe_free_space(directory) {
            Ok(available) => available,
            Err(reason) => {
                return Ok(RollupBackfillReport {
                    progress,
                    terminal: RollupBackfillTerminal::Error(
                        RollupBackfillTerminalError::DiskSpaceProbeFailed { reason },
                    ),
                });
            }
        };
        if available_bytes < required_bytes {
            return Ok(RollupBackfillReport {
                progress,
                terminal: RollupBackfillTerminal::Error(
                    RollupBackfillTerminalError::InsufficientDiskSpace {
                        required_bytes,
                        available_bytes,
                    },
                ),
            });
        }

        progress.phase = RollupBackfillPhase::Folding;
        let previous = state;
        target
            .prepare_chunk(conn, previous, controls.chunk_rows)
            .map_err(RollupBackfillError::FoldChunk)?;
        #[cfg(test)]
        if let Some(hook) = controls.before_chunk_permit {
            hook();
        }
        let chunk = with_ingest_write_permit(|| {
            #[cfg(test)]
            if let Some(hook) = controls.after_chunk_permit {
                hook();
            }
            // Waiting behind maintenance is not chunk work. Start the target's
            // transaction budget only after the shared ingest permit lands.
            let deadline = Instant::now() + controls.chunk_duration();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(RollupBackfillError::BeginChunk)?;
            let chunk = target
                .fold_chunk(
                    &tx,
                    previous,
                    RollupBackfillChunkBudget {
                        max_rows: controls.chunk_rows,
                        deadline,
                    },
                )
                .map_err(RollupBackfillError::FoldChunk)?;
            validate_chunk(&target_name, previous, chunk)?;
            let rows_done = previous
                .rows_done
                .checked_add(chunk.rows_processed)
                .ok_or(RollupBackfillError::RowsDoneOverflow)?;
            let next = RollupBackfillState {
                rows_done,
                done_through: chunk.done_through,
            };
            target
                .persist_chunk(&tx, next, chunk.completed)
                .map_err(RollupBackfillError::PersistChunk)?;
            tx.commit().map_err(RollupBackfillError::CommitChunk)?;
            Ok::<_, RollupBackfillError>((chunk, next))
        })?;

        state = chunk.1;
        progress.phase = RollupBackfillPhase::Checkpointing;
        progress.rows_done = state.rows_done;
        progress.done_through = state.done_through;
        #[cfg(test)]
        if let Some(hook) = controls.before_checkpoint {
            hook(&progress);
        }
        match controls.checkpoint(conn) {
            Ok(RollupCheckpointResult::Complete) => {}
            Ok(RollupCheckpointResult::Busy {
                log_frames,
                checkpointed_frames,
            }) => {
                return Ok(finish(
                    controls,
                    progress,
                    RollupBackfillTerminal::Error(RollupBackfillTerminalError::CheckpointBusy {
                        log_frames,
                        checkpointed_frames,
                    }),
                ));
            }
            Err(reason) => {
                return Ok(finish(
                    controls,
                    progress,
                    RollupBackfillTerminal::Error(RollupBackfillTerminalError::CheckpointFailed {
                        reason,
                    }),
                ));
            }
        }

        progress.phase = RollupBackfillPhase::Folding;
        if let Some(sink) = controls.progress {
            sink(&progress);
        }
        if chunk.0.completed {
            return Ok(RollupBackfillReport {
                progress,
                terminal: RollupBackfillTerminal::Completed,
            });
        }
        if controls
            .after_chunk
            .is_some_and(|hook| hook(&progress) == RollupChunkControl::Interrupt)
        {
            return Ok(RollupBackfillReport {
                progress,
                terminal: RollupBackfillTerminal::Interrupted,
            });
        }
    }
}

fn validate_chunk(
    target: &str,
    previous: RollupBackfillState,
    chunk: RollupBackfillChunk,
) -> Result<(), RollupBackfillError> {
    if !chunk.completed && chunk.rows_processed == 0 {
        return Err(RollupBackfillError::ChunkStalled {
            target: target.to_string(),
            done_through: previous.done_through,
        });
    }
    if chunk.rows_processed > 0 {
        match (previous.done_through, chunk.done_through) {
            (Some(before), Some(after)) if after <= before => {
                return Err(RollupBackfillError::BookmarkDidNotAdvance {
                    target: target.to_string(),
                    before,
                    after,
                });
            }
            (_, None) => {
                return Err(RollupBackfillError::ChunkStalled {
                    target: target.to_string(),
                    done_through: previous.done_through,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn finish(
    controls: &RollupBackfillControls<'_>,
    progress: RollupBackfillProgress,
    terminal: RollupBackfillTerminal,
) -> RollupBackfillReport {
    if let Some(sink) = controls.progress {
        sink(&progress);
    }
    RollupBackfillReport { progress, terminal }
}

fn checkpoint_truncate(conn: &Connection) -> Result<RollupCheckpointResult, String> {
    let (busy, log_frames, checkpointed_frames) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    if busy == 0 {
        Ok(RollupCheckpointResult::Complete)
    } else {
        Ok(RollupCheckpointResult::Busy {
            log_frames,
            checkpointed_frames,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};
    use std::thread;

    use rusqlite::{OptionalExtension, params};
    use serial_test::serial;
    use tempfile::TempDir;

    use super::*;
    use crate::begin_ingest_quiesce;

    struct TestTarget {
        first_chunk_started: Option<Sender<()>>,
        first_chunk_delay: Duration,
    }

    impl TestTarget {
        fn immediate() -> Self {
            Self {
                first_chunk_started: None,
                first_chunk_delay: Duration::ZERO,
            }
        }
    }

    impl RollupBackfillTarget for TestTarget {
        fn name(&self) -> &'static str {
            "test_rollup"
        }

        fn load_state(&self, conn: &Connection) -> rusqlite::Result<RollupBackfillState> {
            conn.query_row(
                "SELECT rows_done, bookmark FROM backfill_meta WHERE id = 1",
                [],
                |row| {
                    Ok(RollupBackfillState {
                        rows_done: row.get(0)?,
                        done_through: row.get(1)?,
                    })
                },
            )
        }

        fn count_total_rows(&self, conn: &Connection) -> rusqlite::Result<u64> {
            conn.query_row("SELECT COUNT(*) FROM raw_rows", [], |row| row.get(0))
        }

        fn fold_chunk(
            &mut self,
            tx: &Transaction<'_>,
            state: RollupBackfillState,
            budget: RollupBackfillChunkBudget,
        ) -> rusqlite::Result<RollupBackfillChunk> {
            if let Some(started) = self.first_chunk_started.take() {
                started.send(()).expect("signal first chunk");
                thread::sleep(self.first_chunk_delay);
            }
            let after = state.done_through.unwrap_or(0);
            let mut statement = tx.prepare(
                "SELECT id, payload FROM raw_rows
                 WHERE id > ?1 ORDER BY id LIMIT ?2",
            )?;
            let rows = statement
                .query_map(params![after, budget.max_rows], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);

            let mut done_through = state.done_through;
            for (id, payload) in &rows {
                tx.execute(
                    "INSERT INTO rolled_rows (id, payload) VALUES (?1, ?2)",
                    params![id, payload],
                )?;
                done_through = Some(*id);
                if budget.should_yield() {
                    break;
                }
            }
            let rows_processed = done_through
                .and_then(|marker| {
                    rows.iter()
                        .position(|(id, _)| *id == marker)
                        .map(|index| index as u64 + 1)
                })
                .unwrap_or(0);
            let remaining = tx
                .query_row(
                    "SELECT 1 FROM raw_rows WHERE id > ?1 LIMIT 1",
                    [done_through.unwrap_or(after)],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            Ok(RollupBackfillChunk {
                rows_processed,
                done_through,
                completed: !remaining,
            })
        }

        fn persist_chunk(
            &self,
            tx: &Transaction<'_>,
            state: RollupBackfillState,
            completed: bool,
        ) -> rusqlite::Result<()> {
            tx.execute(
                "UPDATE backfill_meta
                 SET rows_done = ?1, bookmark = ?2, status = ?3
                 WHERE id = 1",
                params![
                    state.rows_done,
                    state.done_through,
                    if completed { "complete" } else { "running" }
                ],
            )?;
            Ok(())
        }
    }

    fn fixture(rows: i64) -> (TempDir, std::path::PathBuf, Connection) {
        let directory = TempDir::new().expect("temp directory");
        let path = directory.path().join("rollup.db");
        let conn = Connection::open(&path).expect("open fixture");
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE raw_rows (
                 id INTEGER PRIMARY KEY,
                 payload TEXT NOT NULL
             );
             CREATE TABLE rolled_rows (
                 id INTEGER PRIMARY KEY,
                 payload TEXT NOT NULL
             );
             CREATE TABLE backfill_meta (
                 id INTEGER PRIMARY KEY CHECK(id = 1),
                 rows_done INTEGER NOT NULL DEFAULT 0,
                 bookmark INTEGER,
                 status TEXT NOT NULL DEFAULT 'pending'
             );
             INSERT INTO backfill_meta (id) VALUES (1);",
        )
        .expect("create fixture");
        for id in 1..=rows {
            conn.execute(
                "INSERT INTO raw_rows (id, payload) VALUES (?1, ?2)",
                params![id, format!("row-{id}")],
            )
            .expect("seed raw row");
        }
        (directory, path, conn)
    }

    fn roomy(_path: &Path) -> Result<u64, String> {
        Ok(u64::MAX)
    }

    fn controls<'a>() -> RollupBackfillControls<'a> {
        RollupBackfillControls {
            chunk_rows: 2,
            wal_bytes_per_row: 1,
            free_space_multiplier: 1,
            free_space: Some(&roomy),
            ..RollupBackfillControls::default()
        }
    }

    // @lat: [[backend#Backend#Database#Schema#Hourly Analytics Rollups#Chunked Rollup Backfill Framework#Backfill Interrupt And Exact Resume]]
    #[test]
    #[serial]
    fn interrupt_then_resume_is_exactly_once() {
        let (_directory, path, mut conn) = fixture(5);
        let interrupt = |_progress: &RollupBackfillProgress| RollupChunkControl::Interrupt;
        let first_controls = RollupBackfillControls {
            after_chunk: Some(&interrupt),
            ..controls()
        };

        let first = run_rollup_backfill(
            &mut conn,
            &path,
            &mut TestTarget::immediate(),
            &first_controls,
        )
        .expect("interrupt first run");
        assert_eq!(RollupBackfillTerminal::Interrupted, first.terminal);
        assert_eq!(2, first.progress.rows_done);
        assert_eq!(Some(2), first.progress.done_through);

        let resumed =
            run_rollup_backfill(&mut conn, &path, &mut TestTarget::immediate(), &controls())
                .expect("resume backfill");
        assert_eq!(RollupBackfillTerminal::Completed, resumed.terminal);
        assert_eq!(5, resumed.progress.rows_done);
        assert_eq!(Some(5), resumed.progress.done_through);
        let missing_or_duplicate: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM (
                     SELECT id, payload FROM raw_rows
                     EXCEPT
                     SELECT id, payload FROM rolled_rows
                 )",
                [],
                |row| row.get(0),
            )
            .expect("compare raw and rollup");
        assert_eq!(0, missing_or_duplicate);
        assert_eq!(
            5,
            conn.query_row("SELECT COUNT(*) FROM rolled_rows", [], |row| row
                .get::<_, i64>(0))
                .expect("count rollup rows")
        );
    }

    // @lat: [[backend#Backend#Database#Schema#Hourly Analytics Rollups#Chunked Rollup Backfill Framework#Maintenance Lease Acquires Between Chunks]]
    #[test]
    #[serial]
    fn queued_maintenance_acquires_within_one_chunk_bound() {
        let (_directory, path, conn) = fixture(4);
        let observer = Connection::open(&path).expect("open observer");
        let (started_tx, started_rx) = mpsc::channel();
        let worker_path = path.clone();
        let worker = thread::spawn(move || {
            let mut conn = conn;
            let mut target = TestTarget {
                first_chunk_started: Some(started_tx),
                first_chunk_delay: Duration::from_millis(100),
            };
            let lease_controls = RollupBackfillControls {
                chunk_rows: 1,
                ..controls()
            };
            run_rollup_backfill(&mut conn, &worker_path, &mut target, &lease_controls)
                .expect("run concurrent backfill")
        });

        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first chunk starts");
        let queued_at = Instant::now();
        let lease = begin_ingest_quiesce();
        let acquisition = queued_at.elapsed();
        eprintln!("maintenance lease acquisition: {acquisition:?}");
        assert!(
            acquisition <= ROLLUP_BACKFILL_CHUNK_TARGET,
            "maintenance waited {acquisition:?}"
        );
        assert_eq!(
            1,
            observer
                .query_row("SELECT COUNT(*) FROM rolled_rows", [], |row| row
                    .get::<_, i64>(0))
                .expect("count first committed chunk")
        );
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            1,
            observer
                .query_row("SELECT COUNT(*) FROM rolled_rows", [], |row| row
                    .get::<_, i64>(0))
                .expect("backfill remains yielded")
        );
        drop(lease);

        let report = worker.join().expect("join backfill");
        assert_eq!(RollupBackfillTerminal::Completed, report.terminal);
    }

    // @lat: [[backend#Backend#Database#Schema#Hourly Analytics Rollups#Chunked Rollup Backfill Framework#Disk And Checkpoint Failures Stop Safely]]
    #[test]
    #[serial]
    fn disk_and_checkpoint_failures_return_typed_terminals() {
        let (_directory, path, mut conn) = fixture(5);
        let starved = |_path: &Path| Ok(0);
        let disk_controls = RollupBackfillControls {
            free_space: Some(&starved),
            ..controls()
        };
        let disk = run_rollup_backfill(
            &mut conn,
            &path,
            &mut TestTarget::immediate(),
            &disk_controls,
        )
        .expect("disk refusal");
        assert_eq!(
            RollupBackfillTerminal::Error(RollupBackfillTerminalError::InsufficientDiskSpace {
                required_bytes: 2,
                available_bytes: 0,
            }),
            disk.terminal
        );
        assert_eq!(0, disk.progress.rows_done);
        assert_eq!(None, disk.progress.done_through);

        let busy = |_conn: &Connection| {
            Ok(RollupCheckpointResult::Busy {
                log_frames: 3,
                checkpointed_frames: 1,
            })
        };
        let busy_controls = RollupBackfillControls {
            checkpoint: Some(&busy),
            ..controls()
        };
        let busy_report = run_rollup_backfill(
            &mut conn,
            &path,
            &mut TestTarget::immediate(),
            &busy_controls,
        )
        .expect("checkpoint busy");
        assert_eq!(
            RollupBackfillTerminal::Error(RollupBackfillTerminalError::CheckpointBusy {
                log_frames: 3,
                checkpointed_frames: 1,
            }),
            busy_report.terminal
        );
        assert_eq!(2, busy_report.progress.rows_done);
        assert_eq!(Some(2), busy_report.progress.done_through);

        let failed = |_conn: &Connection| Err("checkpoint I/O error".to_string());
        let failed_controls = RollupBackfillControls {
            checkpoint: Some(&failed),
            ..controls()
        };
        let failed_report = run_rollup_backfill(
            &mut conn,
            &path,
            &mut TestTarget::immediate(),
            &failed_controls,
        )
        .expect("checkpoint failure");
        assert_eq!(
            RollupBackfillTerminal::Error(RollupBackfillTerminalError::CheckpointFailed {
                reason: "checkpoint I/O error".to_string(),
            }),
            failed_report.terminal
        );
        assert_eq!(4, failed_report.progress.rows_done);
        assert_eq!(Some(4), failed_report.progress.done_through);

        let completed =
            run_rollup_backfill(&mut conn, &path, &mut TestTarget::immediate(), &controls())
                .expect("resume after checkpoint failures");
        assert_eq!(RollupBackfillTerminal::Completed, completed.terminal);
        assert_eq!(5, completed.progress.rows_done);
    }

    #[test]
    #[serial]
    fn relative_database_path_probes_current_directory() {
        let (_directory, _path, mut conn) = fixture(0);
        let current_directory = |path: &Path| {
            assert_eq!(path, Path::new("."));
            Ok(u64::MAX)
        };
        let controls = RollupBackfillControls {
            free_space: Some(&current_directory),
            ..controls()
        };
        let report = run_rollup_backfill(
            &mut conn,
            Path::new("relative.db"),
            &mut TestTarget::immediate(),
            &controls,
        )
        .expect("run relative-path backfill");
        assert_eq!(report.terminal, RollupBackfillTerminal::Completed);
    }
}
