//! Frozen synthetic corpus for retention pruning (feature 014).
//!
//! Every retention acceptance test runs against **this one builder**, keeping
//! cutoff, shape, and row-count assertions on one corpus. The historical
//! timing evidence was captured from the same fixture before its one-off
//! measurement binary was removed.
//!
//! # Contract
//!
//! [`build_retention_fixture`] follows the storage test harness exactly: it
//! sets `QUILL_DEMO_MODE=1` and `QUILL_DATA_DIR` to a fresh [`TempDir`] and
//! *then* calls `Storage::init`, so the database is created by the real
//! migration path and carries every index a production database has. The env
//! block is process-global, so **every test that calls this builder must be
//! annotated `#[serial]`** (`serial_test`) or concurrent tests will race each
//! other into the wrong database.
//!
//! The overrides are deliberately left set after the call, matching
//! `storage::tests::init_storage_in`: a consumer that wants a `Storage` handle
//! onto the fixture database calls `Storage::init()` again and lands in the
//! same temp directory.
//!
//! # Shape of the corpus
//!
//! Rows are laid out in fixed 30-day *buckets* counted back from an anchor
//! instant, bucket 0 being the most recent. Each bucket holds, for each of the
//! five source-owned analytics tables:
//!
//! * `owned_rows_per_month` source-owned rows with **conforming** timestamps
//!   (`length = 24 AND LIKE '%Z'`, the guard retention filters on),
//! * `live_rows_per_month` live rows with `source_key IS NULL`, which
//!   retention never touches, and
//! * for the two retention *target* tables only, one row per
//!   [`NonConformingShape`] — timestamps retention must retain and report
//!   rather than delete.
//!
//! Every row's instant is derived arithmetically from its bucket, table and
//! kind, so per-month counts are exact and every cutoff produced by
//! [`RetentionFixturePlan::boundary`] falls cleanly between two buckets. That
//! makes "the run deleted exactly the pre-cutoff rows and no others" an exact
//! equality rather than a range check.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SubsecRound, TimeDelta, Utc};
use rusqlite::{Connection, Statement, Transaction, params};
use tempfile::TempDir;

use crate::storage::Storage;

/// Provider every fixture row is attributed to. One provider keeps the
/// per-month counts trivially assertable; retention is provider-agnostic.
pub const FIXTURE_PROVIDER: &str = "claude";
const NONCONFORMING_RUNTIME_SOURCE: &str = "retention-fixture/nonconforming-runtime-source.jsonl";

/// Width of one synthetic "month" bucket, in days. Fixed rather than calendar
/// months so bucket boundaries are exact multiples of a duration.
pub const BUCKET_DAYS: i64 = 30;

/// Seconds reserved per row index inside a bucket. Each (table, kind) pair
/// occupies its own slot within the stride so no two rows in a bucket ever
/// share an instant, which keeps every partial unique index satisfied.
const BUCKET_SLOTS: i64 = 16;

const SECONDS_PER_BUCKET: i64 = BUCKET_DAYS * 86_400;

/// Largest `*_rows_per_month` a spec may request. Beyond this the oldest row
/// in a bucket would fall outside the bucket and break the boundary math.
pub const MAX_ROWS_PER_MONTH: u32 = ((SECONDS_PER_BUCKET - BUCKET_SLOTS) / BUCKET_SLOTS) as u32;

/// The three malformed timestamp shapes the corpus plants in both retention
/// target tables, one row of each per bucket.
pub const NON_CONFORMING_SHAPES: [NonConformingShape; 3] = [
    NonConformingShape::UtcOffsetSuffix,
    NonConformingShape::SecondsPrecision,
    NonConformingShape::NoTrailingZulu,
];

/// A source-owned analytics table the fixture populates.
///
/// [`RetentionTable::ToolActions`] and [`RetentionTable::SessionEvents`] are
/// retention's targets; the other three are siblings that must survive a run
/// untouched, which is only assertable if the fixture actually fills them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RetentionTable {
    ToolActions,
    SessionEvents,
    ResponseTimes,
    SkillUsages,
    HookInvocations,
}

impl RetentionTable {
    /// Every table the fixture populates, in a fixed order.
    pub const ALL: [RetentionTable; 5] = [
        RetentionTable::ToolActions,
        RetentionTable::SessionEvents,
        RetentionTable::ResponseTimes,
        RetentionTable::SkillUsages,
        RetentionTable::HookInvocations,
    ];

    /// SQL table name.
    pub const fn as_str(self) -> &'static str {
        match self {
            RetentionTable::ToolActions => "tool_actions",
            RetentionTable::SessionEvents => "session_events",
            RetentionTable::ResponseTimes => "response_times",
            RetentionTable::SkillUsages => "skill_usages",
            RetentionTable::HookInvocations => "hook_invocations",
        }
    }

    /// Whether age-based retention deletes from this table.
    pub const fn is_retention_target(self) -> bool {
        matches!(
            self,
            RetentionTable::ToolActions | RetentionTable::SessionEvents
        )
    }

    const fn lane(self) -> i64 {
        match self {
            RetentionTable::ToolActions => 0,
            RetentionTable::SessionEvents => 1,
            RetentionTable::ResponseTimes => 2,
            RetentionTable::SkillUsages => 3,
            RetentionTable::HookInvocations => 4,
        }
    }

    const fn key_prefix(self) -> &'static str {
        match self {
            RetentionTable::ToolActions => "ta",
            RetentionTable::SessionEvents => "se",
            RetentionTable::ResponseTimes => "rt",
            RetentionTable::SkillUsages => "su",
            RetentionTable::HookInvocations => "hi",
        }
    }
}

/// The three populations the corpus distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RetentionRowKind {
    /// Source-owned rows with a conforming timestamp — retention's doomed set.
    OwnedConforming,
    /// Source-owned rows whose timestamp fails the conformance guard.
    OwnedNonConforming,
    /// `source_key IS NULL` rows, which retention excludes entirely.
    Live,
}

impl RetentionRowKind {
    /// Every population, in a fixed order.
    pub const ALL: [RetentionRowKind; 3] = [
        RetentionRowKind::OwnedConforming,
        RetentionRowKind::OwnedNonConforming,
        RetentionRowKind::Live,
    ];

    /// SQL predicate selecting exactly this population.
    ///
    /// The conformance halves are spelled out here so a test can assert the
    /// corpus really does straddle the guard retention will apply.
    pub const fn predicate(self) -> &'static str {
        match self {
            RetentionRowKind::OwnedConforming => {
                "source_key IS NOT NULL AND length(timestamp) = 24 AND timestamp LIKE '%Z'"
            }
            RetentionRowKind::OwnedNonConforming => {
                "source_key IS NOT NULL AND NOT (length(timestamp) = 24 AND timestamp LIKE '%Z')"
            }
            RetentionRowKind::Live => "source_key IS NULL",
        }
    }

    const fn lane(self) -> i64 {
        match self {
            RetentionRowKind::OwnedConforming => 0,
            RetentionRowKind::OwnedNonConforming => 1,
            RetentionRowKind::Live => 2,
        }
    }

    const fn key_tag(self) -> &'static str {
        match self {
            RetentionRowKind::OwnedConforming => "o",
            RetentionRowKind::OwnedNonConforming => "n",
            RetentionRowKind::Live => "l",
        }
    }
}

/// A malformed timestamp shape that must survive retention.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NonConformingShape {
    /// `2026-04-25T00:00:00.000+00:00` — 29 chars, fails both guard halves.
    UtcOffsetSuffix,
    /// `2026-04-25T00:00:00Z` — 20 chars, carries `Z`, fails the length half.
    SecondsPrecision,
    /// `2026-04-25T00:00:00+0000` — 24 chars, fails only the `LIKE '%Z'` half.
    NoTrailingZulu,
}

/// Knobs for one fixture build.
#[derive(Clone, Debug)]
pub struct RetentionFixtureSpec {
    /// Newest instant in the corpus. Bucket boundaries are measured back from
    /// here, so callers that want a stable cutoff pass a fixed anchor.
    pub anchor: DateTime<Utc>,
    /// Number of 30-day buckets to generate.
    pub months: u32,
    /// Source-owned conforming rows per table per bucket.
    pub owned_rows_per_month: u32,
    /// `source_key IS NULL` rows per table per bucket.
    pub live_rows_per_month: u32,
    /// Distinct `source_key` values owned rows are spread across.
    pub sources: u32,
}

impl Default for RetentionFixtureSpec {
    fn default() -> Self {
        Self {
            anchor: Utc::now(),
            months: 6,
            owned_rows_per_month: 12,
            live_rows_per_month: 4,
            sources: 3,
        }
    }
}

/// One 30-day bucket and the exact number of rows it holds.
#[derive(Clone, Debug)]
pub struct RetentionMonthBucket {
    /// 0 is the most recent bucket.
    pub index: u32,
    /// Newer bound, exclusive. Equal to `plan.boundary(index)`.
    pub end: DateTime<Utc>,
    /// Owned conforming rows in this bucket, per owned table.
    pub owned_conforming_rows: u64,
    /// Owned non-conforming rows in this bucket, per retention target table.
    pub owned_non_conforming_rows: u64,
    /// Live rows in this bucket, per table.
    pub live_rows: u64,
}

impl RetentionMonthBucket {
    /// Exact row count this bucket contributes for one table and population.
    pub const fn rows(&self, table: RetentionTable, kind: RetentionRowKind) -> u64 {
        match kind {
            RetentionRowKind::OwnedConforming => self.owned_conforming_rows,
            RetentionRowKind::Live => self.live_rows,
            RetentionRowKind::OwnedNonConforming => {
                if table.is_retention_target() {
                    self.owned_non_conforming_rows
                } else {
                    0
                }
            }
        }
    }
}

/// The frozen, fully enumerable description of what the corpus contains.
///
/// Consumers assert against this rather than against hand-copied literals, so
/// changing a spec knob cannot silently invalidate a test's expectations.
#[derive(Clone, Debug)]
pub struct RetentionFixturePlan {
    anchor: DateTime<Utc>,
    sources: u32,
    buckets: Vec<RetentionMonthBucket>,
}

impl RetentionFixturePlan {
    /// Newest instant in the corpus.
    pub fn anchor(&self) -> DateTime<Utc> {
        self.anchor
    }

    /// Number of buckets generated.
    pub fn months(&self) -> u32 {
        self.buckets.len() as u32
    }

    /// Distinct `source_key` values owned rows are spread across.
    pub fn sources(&self) -> u32 {
        self.sources
    }

    /// Buckets, most recent first.
    pub fn buckets(&self) -> &[RetentionMonthBucket] {
        &self.buckets
    }

    /// Cutoff instant that retains `months_retained` buckets.
    ///
    /// No generated row ever lands exactly on a boundary, so `< boundary` and
    /// `<= boundary` select the same rows and a boundary-off-by-one in the
    /// delete engine still shows up as a count mismatch elsewhere.
    pub fn boundary(&self, months_retained: u32) -> DateTime<Utc> {
        self.anchor - TimeDelta::days(BUCKET_DAYS * i64::from(months_retained))
    }

    /// [`RetentionFixturePlan::boundary`] rendered in the conforming format,
    /// ready to bind as a SQL cutoff literal.
    pub fn boundary_timestamp(&self, months_retained: u32) -> String {
        conforming_timestamp(self.boundary(months_retained))
    }

    /// Total rows of one population in one table across the whole corpus.
    pub fn total_rows(&self, table: RetentionTable, kind: RetentionRowKind) -> u64 {
        self.buckets
            .iter()
            .map(|bucket| bucket.rows(table, kind))
            .sum()
    }

    /// Rows of one population in one table strictly older than
    /// [`RetentionFixturePlan::boundary`] for `months_retained`.
    ///
    /// For [`RetentionRowKind::OwnedConforming`] on a retention target table
    /// this is exactly the number of rows a correct run must delete.
    pub fn rows_before_boundary(
        &self,
        months_retained: u32,
        table: RetentionTable,
        kind: RetentionRowKind,
    ) -> u64 {
        self.buckets
            .iter()
            .filter(|bucket| bucket.index >= months_retained)
            .map(|bucket| bucket.rows(table, kind))
            .sum()
    }
}

/// A built fixture: a temp data directory, the database inside it, and the
/// plan describing exactly what was written.
///
/// Dropping this removes the temp directory, so hold it for the whole test.
pub struct RetentionFixture {
    _data_dir: TempDir,
    db_path: PathBuf,
    plan: RetentionFixturePlan,
}

impl RetentionFixture {
    /// Path of the SQLite database `Storage::init` created.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// What the corpus contains.
    pub fn plan(&self) -> &RetentionFixturePlan {
        &self.plan
    }

    /// Open a dedicated read/write connection onto the fixture database.
    ///
    /// Deliberately separate from any `Storage` handle, mirroring how the
    /// maintenance path opens its own connection.
    pub fn open_connection(&self) -> Result<Connection, RetentionFixtureError> {
        let conn = Connection::open(&self.db_path)?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
        Ok(conn)
    }
}

/// Everything that can go wrong building the fixture.
#[derive(Debug)]
pub enum RetentionFixtureError {
    /// The temp data directory could not be created or canonicalized.
    DataDir(std::io::Error),
    /// `Storage::init` refused to create or migrate the database.
    StorageInit(String),
    /// A SQLite operation failed while writing the corpus.
    Sqlite(rusqlite::Error),
    /// A spec knob was zero where the corpus needs at least one.
    EmptySpec { field: &'static str },
    /// A per-month row count would push rows outside their own bucket.
    BucketCapacity { requested: u32, maximum: u32 },
}

impl std::fmt::Display for RetentionFixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetentionFixtureError::DataDir(error) => {
                write!(f, "Create retention fixture data directory: {error}")
            }
            RetentionFixtureError::StorageInit(error) => {
                write!(f, "Initialize retention fixture database: {error}")
            }
            RetentionFixtureError::Sqlite(error) => {
                write!(f, "Write retention fixture corpus: {error}")
            }
            RetentionFixtureError::EmptySpec { field } => {
                write!(f, "Retention fixture spec field `{field}` must be non-zero")
            }
            RetentionFixtureError::BucketCapacity { requested, maximum } => write!(
                f,
                "Retention fixture requested {requested} rows per month, which exceeds the \
                 per-bucket maximum of {maximum}"
            ),
        }
    }
}

impl std::error::Error for RetentionFixtureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RetentionFixtureError::DataDir(error) => Some(error),
            RetentionFixtureError::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for RetentionFixtureError {
    fn from(error: rusqlite::Error) -> Self {
        RetentionFixtureError::Sqlite(error)
    }
}

/// Render an instant in the format retention treats as conforming: exactly 24
/// characters, millisecond precision, trailing `Z`.
pub fn conforming_timestamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Render an instant in one of the malformed shapes retention must retain.
pub fn non_conforming_timestamp(at: DateTime<Utc>, shape: NonConformingShape) -> String {
    match shape {
        NonConformingShape::UtcOffsetSuffix => at.format("%Y-%m-%dT%H:%M:%S%.3f+00:00").to_string(),
        NonConformingShape::SecondsPrecision => at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        NonConformingShape::NoTrailingZulu => at.format("%Y-%m-%dT%H:%M:%S+0000").to_string(),
    }
}

/// Count the rows of one population currently in one table.
pub fn count_rows(
    conn: &Connection,
    table: RetentionTable,
    kind: RetentionRowKind,
) -> Result<u64, RetentionFixtureError> {
    // Both interpolated fragments are compile-time constants from this
    // module's own enums; nothing caller-supplied reaches the SQL text.
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE {}",
        table.as_str(),
        kind.predicate()
    );
    let count: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
    Ok(count.max(0) as u64)
}

/// Count the rows of one population in one table strictly older than `cutoff`.
pub fn count_rows_before(
    conn: &Connection,
    table: RetentionTable,
    kind: RetentionRowKind,
    cutoff: &str,
) -> Result<u64, RetentionFixtureError> {
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE {} AND timestamp < ?1",
        table.as_str(),
        kind.predicate()
    );
    let count: i64 = conn.query_row(&sql, params![cutoff], |row| row.get(0))?;
    Ok(count.max(0) as u64)
}

/// Build the frozen synthetic retention corpus.
///
/// Sets `QUILL_DEMO_MODE` and `QUILL_DATA_DIR` to a fresh temp directory,
/// creates the database through `Storage::init` so every migration index
/// exists, then writes the corpus over a dedicated connection.
///
/// # Safety contract
///
/// This mutates process-global environment variables. Callers in a test binary
/// **must** be annotated `#[serial]`.
pub fn build_retention_fixture(
    spec: &RetentionFixtureSpec,
) -> Result<RetentionFixture, RetentionFixtureError> {
    validate_spec(spec)?;

    let data_dir = TempDir::new().map_err(RetentionFixtureError::DataDir)?;
    let canonical_dir =
        std::fs::canonicalize(data_dir.path()).map_err(RetentionFixtureError::DataDir)?;

    // SAFETY: environment mutation is process-global; the module contract
    // requires every consuming test to hold the `#[serial]` lock. The
    // overrides are intentionally left set so a consumer can call
    // `Storage::init()` again and land in this same directory.
    unsafe {
        std::env::set_var("QUILL_DEMO_MODE", "1");
        std::env::set_var("QUILL_DATA_DIR", &canonical_dir);
    }

    // The schema must come from the real migration path — a hand-written
    // CREATE TABLE would silently miss the partial unique indexes the delete
    // engine's query plans depend on.
    let storage = Storage::init().map_err(RetentionFixtureError::StorageInit)?;
    drop(storage);

    let db_path = canonical_dir.join("usage.db");
    let plan = build_plan(spec);

    let mut conn = Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
    write_corpus(&mut conn, &plan)?;
    drop(conn);

    Ok(RetentionFixture {
        _data_dir: data_dir,
        db_path,
        plan,
    })
}

fn validate_spec(spec: &RetentionFixtureSpec) -> Result<(), RetentionFixtureError> {
    if spec.months == 0 {
        return Err(RetentionFixtureError::EmptySpec { field: "months" });
    }
    if spec.sources == 0 {
        return Err(RetentionFixtureError::EmptySpec { field: "sources" });
    }
    let widest = spec
        .owned_rows_per_month
        .max(spec.live_rows_per_month)
        .max(NON_CONFORMING_SHAPES.len() as u32);
    if widest > MAX_ROWS_PER_MONTH {
        return Err(RetentionFixtureError::BucketCapacity {
            requested: widest,
            maximum: MAX_ROWS_PER_MONTH,
        });
    }
    Ok(())
}

fn build_plan(spec: &RetentionFixtureSpec) -> RetentionFixturePlan {
    let anchor = spec.anchor.trunc_subsecs(0);
    let buckets = (0..spec.months)
        .map(|index| {
            let end = anchor - TimeDelta::days(BUCKET_DAYS * i64::from(index));
            RetentionMonthBucket {
                index,
                end,
                owned_conforming_rows: u64::from(spec.owned_rows_per_month),
                owned_non_conforming_rows: NON_CONFORMING_SHAPES.len() as u64,
                live_rows: u64::from(spec.live_rows_per_month),
            }
        })
        .collect();

    RetentionFixturePlan {
        anchor,
        sources: spec.sources,
        buckets,
    }
}

/// Number of table lanes inside one row-kind band. Kept as a literal so the
/// slot arithmetic stays a `const fn`.
const TABLE_LANES: i64 = 5;

const fn slot_for(table: RetentionTable, kind: RetentionRowKind) -> i64 {
    kind.lane() * TABLE_LANES + table.lane()
}

/// Identity and timing of a single generated row, before it is bound to a
/// table-specific statement.
struct RowSeed {
    timestamp: String,
    source_key: Option<String>,
    session_id: String,
    chain_id: String,
    key: String,
}

fn write_corpus(
    conn: &mut Connection,
    plan: &RetentionFixturePlan,
) -> Result<(), RetentionFixtureError> {
    let tx = conn.transaction()?;
    write_sources(&tx, plan)?;
    for table in RetentionTable::ALL {
        write_table(&tx, plan, table)?;
    }
    for index in 0..plan.sources() {
        crate::storage::refold_runtime_source_identity(
            &tx,
            FIXTURE_PROVIDER,
            &source_key(index),
            &format!("session-{index:04}"),
            &format!("chain-{index:04}"),
        )
        .map_err(RetentionFixtureError::StorageInit)?;
    }
    crate::storage::refold_runtime_source_identity(
        &tx,
        FIXTURE_PROVIDER,
        NONCONFORMING_RUNTIME_SOURCE,
        "nonconforming-runtime-session",
        "nonconforming-runtime-chain",
    )
    .map_err(RetentionFixtureError::StorageInit)?;
    tx.commit()?;
    Ok(())
}

fn source_key(index: u32) -> String {
    format!("retention-fixture/source-{index:04}.jsonl")
}

fn write_sources(
    tx: &Transaction<'_>,
    plan: &RetentionFixturePlan,
) -> Result<(), RetentionFixtureError> {
    let mut stmt = tx.prepare(
        "INSERT INTO transcript_analytics_sources
             (provider, source_key, source_root_key, source_path,
              analytics_session_id, chain_id, seen_generation, processing_status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 'ok')",
    )?;
    for index in 0..plan.sources() {
        let key = source_key(index);
        stmt.execute(params![
            FIXTURE_PROVIDER,
            key,
            "retention-fixture",
            format!("/retention-fixture/source-{index:04}.jsonl"),
            format!("session-{index:04}"),
            format!("chain-{index:04}"),
        ])?;
    }
    stmt.execute(params![
        FIXTURE_PROVIDER,
        NONCONFORMING_RUNTIME_SOURCE,
        "retention-fixture",
        "/retention-fixture/nonconforming-runtime-source.jsonl",
        "nonconforming-runtime-session",
        "nonconforming-runtime-chain",
    ])?;
    Ok(())
}

fn write_table(
    tx: &Transaction<'_>,
    plan: &RetentionFixturePlan,
    table: RetentionTable,
) -> Result<(), RetentionFixtureError> {
    let mut stmt = tx.prepare(insert_sql(table))?;
    if table == RetentionTable::SessionEvents {
        let mut seeds = Vec::new();
        for bucket in plan.buckets() {
            for kind in RetentionRowKind::ALL {
                let rows = bucket.rows(table, kind);
                let slot = slot_for(table, kind);
                for row in 0..rows {
                    seeds.push(seed_for(plan, bucket, table, kind, slot, row));
                }
            }
        }
        seeds.sort_by_key(|seed| {
            DateTime::parse_from_rfc3339(&seed.timestamp)
                .map(|timestamp| timestamp.timestamp_millis())
                .unwrap_or(i64::MAX)
        });
        for seed in &seeds {
            bind_row(&mut stmt, seed)?;
        }
        return Ok(());
    }
    for bucket in plan.buckets() {
        for kind in RetentionRowKind::ALL {
            let rows = bucket.rows(table, kind);
            let slot = slot_for(table, kind);
            for row in 0..rows {
                let seed = seed_for(plan, bucket, table, kind, slot, row);
                bind_row(&mut stmt, &seed)?;
            }
        }
    }
    Ok(())
}

fn seed_for(
    plan: &RetentionFixturePlan,
    bucket: &RetentionMonthBucket,
    table: RetentionTable,
    kind: RetentionRowKind,
    slot: i64,
    row: u64,
) -> RowSeed {
    let seconds_back = 1 + (row as i64) * BUCKET_SLOTS + slot;
    let instant = bucket.end - TimeDelta::seconds(seconds_back);
    let timestamp = match kind {
        RetentionRowKind::OwnedNonConforming => non_conforming_timestamp(
            instant,
            NON_CONFORMING_SHAPES[(row as usize) % NON_CONFORMING_SHAPES.len()],
        ),
        _ => conforming_timestamp(instant),
    };
    let key = format!(
        "{}-{}-b{:04}-{:06}",
        table.key_prefix(),
        kind.key_tag(),
        bucket.index,
        row
    );

    match kind {
        RetentionRowKind::Live => RowSeed {
            timestamp,
            source_key: None,
            session_id: format!("live-session-{:04}", bucket.index),
            chain_id: format!("live-chain-{:04}", bucket.index),
            key,
        },
        _ => {
            if table == RetentionTable::SessionEvents
                && kind == RetentionRowKind::OwnedNonConforming
            {
                return RowSeed {
                    timestamp,
                    source_key: Some(NONCONFORMING_RUNTIME_SOURCE.to_string()),
                    session_id: "nonconforming-runtime-session".to_string(),
                    chain_id: "nonconforming-runtime-chain".to_string(),
                    key,
                };
            }
            let source_index = ((u64::from(bucket.index) + row) % u64::from(plan.sources())) as u32;
            RowSeed {
                timestamp,
                source_key: Some(source_key(source_index)),
                session_id: format!("session-{source_index:04}"),
                chain_id: format!("chain-{source_index:04}"),
                key,
            }
        }
    }
}

const fn insert_sql(table: RetentionTable) -> &'static str {
    match table {
        RetentionTable::ToolActions => {
            "INSERT INTO tool_actions
                 (provider, source_key, action_key, message_id, session_id, chain_id,
                  tool_name, category, summary, timestamp)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5, 'Read', 'tool_detail', ?3, ?6)"
        }
        RetentionTable::SessionEvents => {
            "INSERT INTO session_events
                 (provider, source_key, event_key, session_id, chain_id, timestamp, kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'user_prompt')"
        }
        RetentionTable::ResponseTimes => {
            "INSERT INTO response_times
                 (provider, source_key, session_id, chain_id, timestamp,
                  response_secs, idle_secs)
             VALUES (?1, ?2, ?4, ?5, ?6, 1.5, 0.5)"
        }
        RetentionTable::SkillUsages => {
            "INSERT INTO skill_usages
                 (provider, source_key, session_id, chain_id, message_id,
                  skill_name, skill_path, timestamp)
             VALUES (?1, ?2, ?4, ?5, ?3, 'retention-fixture', '/skills/retention', ?6)"
        }
        RetentionTable::HookInvocations => {
            "INSERT INTO hook_invocations
                 (provider, source_key, session_id, chain_id, timestamp,
                  hook_event, hook_identity)
             VALUES (?1, ?2, ?4, ?5, ?6, 'PreToolUse', ?3)"
        }
    }
}

/// Bind one seed against a table statement.
///
/// Every statement in [`insert_sql`] takes the same six positional parameters
/// in the same order, so one binder serves all five tables; a table that does
/// not need a slot simply omits that placeholder from its SQL.
fn bind_row(stmt: &mut Statement<'_>, seed: &RowSeed) -> Result<(), RetentionFixtureError> {
    stmt.execute(params![
        FIXTURE_PROVIDER,
        seed.source_key,
        seed.key,
        seed.session_id,
        seed.chain_id,
        seed.timestamp,
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn small_spec() -> RetentionFixtureSpec {
        RetentionFixtureSpec {
            // A fixed anchor keeps every boundary literal in these assertions
            // stable regardless of when the suite runs.
            anchor: DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
                .expect("parse anchor")
                .with_timezone(&Utc),
            months: 4,
            owned_rows_per_month: 5,
            live_rows_per_month: 2,
            sources: 3,
        }
    }

    // @lat: [[backend#Backend#Database#Retention fixture#Retention Fixture Test Specs#Exact Per-Month Counts]]
    #[test]
    #[serial]
    fn fixture_writes_exact_per_month_counts() {
        let fixture = build_retention_fixture(&small_spec()).expect("build fixture");
        let conn = fixture.open_connection().expect("open connection");
        let plan = fixture.plan();

        for table in RetentionTable::ALL {
            for kind in RetentionRowKind::ALL {
                let expected = plan.total_rows(table, kind);
                let actual = count_rows(&conn, table, kind).expect("count rows");
                assert_eq!(expected, actual, "{} / {kind:?} row count", table.as_str());
            }
        }

        // Siblings carry no planted non-conformance, so the corpus can prove a
        // run left them alone without a conformance caveat.
        for table in RetentionTable::ALL {
            if !table.is_retention_target() {
                assert_eq!(
                    0,
                    plan.total_rows(table, RetentionRowKind::OwnedNonConforming),
                    "{} must have no non-conforming rows",
                    table.as_str()
                );
            }
        }
    }

    // @lat: [[backend#Backend#Database#Retention fixture#Retention Fixture Test Specs#Boundary Row Split]]
    #[test]
    #[serial]
    fn boundary_splits_owned_rows_exactly() {
        let fixture = build_retention_fixture(&small_spec()).expect("build fixture");
        let conn = fixture.open_connection().expect("open connection");
        let plan = fixture.plan();

        for months_retained in 0..=plan.months() {
            let cutoff = plan.boundary_timestamp(months_retained);
            for table in RetentionTable::ALL {
                let expected = plan.rows_before_boundary(
                    months_retained,
                    table,
                    RetentionRowKind::OwnedConforming,
                );
                let actual =
                    count_rows_before(&conn, table, RetentionRowKind::OwnedConforming, &cutoff)
                        .expect("count rows before cutoff");
                assert_eq!(
                    expected,
                    actual,
                    "{} owned conforming rows older than {cutoff}",
                    table.as_str()
                );
            }
        }
    }

    // @lat: [[backend#Backend#Database#Retention fixture#Retention Fixture Test Specs#Guard Straddling Rows]]
    #[test]
    #[serial]
    fn live_and_non_conforming_rows_straddle_the_cutoff() {
        let fixture = build_retention_fixture(&small_spec()).expect("build fixture");
        let conn = fixture.open_connection().expect("open connection");
        let plan = fixture.plan();
        let months_retained = 2;
        let cutoff = plan.boundary_timestamp(months_retained);

        for table in RetentionTable::ALL {
            let older = count_rows_before(&conn, table, RetentionRowKind::Live, &cutoff)
                .expect("count live rows before cutoff");
            let total = count_rows(&conn, table, RetentionRowKind::Live).expect("count live rows");
            assert!(
                older > 0,
                "{} needs live rows older than cutoff",
                table.as_str()
            );
            assert!(
                total > older,
                "{} needs live rows newer than cutoff",
                table.as_str()
            );
        }

        for table in RetentionTable::ALL {
            if !table.is_retention_target() {
                continue;
            }
            let total =
                count_rows(&conn, table, RetentionRowKind::OwnedNonConforming).expect("count");
            assert_eq!(
                u64::from(plan.months()) * NON_CONFORMING_SHAPES.len() as u64,
                total,
                "{} non-conforming row count",
                table.as_str()
            );
        }

        // Each shape must actually be present, or a guard bug in one half of
        // `length = 24 AND LIKE '%Z'` could pass unnoticed.
        for shape in NON_CONFORMING_SHAPES {
            let sample = non_conforming_timestamp(plan.anchor(), shape);
            assert!(
                !(sample.len() == 24 && sample.ends_with('Z')),
                "shape {shape:?} rendered a conforming timestamp: {sample}"
            );
        }
        assert_eq!(24, conforming_timestamp(plan.anchor()).len());
    }

    // @lat: [[backend#Backend#Database#Retention fixture#Retention Fixture Test Specs#Migrated Schema And Reopen]]
    #[test]
    #[serial]
    fn fixture_database_carries_migrated_indexes_and_reopens() {
        let fixture = build_retention_fixture(&small_spec()).expect("build fixture");
        let conn = fixture.open_connection().expect("open connection");

        // Deliberately probes indexes created by migration 30 *and*
        // `idx_se_timestamp_chain`, which only `ensure_startup_indexes`
        // creates — together they prove the whole `Storage::init` path ran,
        // not just a bare `CREATE TABLE`.
        for index in [
            "uidx_ta_owned",
            "uidx_ta_live",
            "uidx_se_owned",
            "uidx_se_live",
            "idx_se_timestamp",
            "idx_se_timestamp_chain",
        ] {
            let present: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    params![index],
                    |row| row.get(0),
                )
                .expect("probe index");
            assert_eq!(1, present, "migrated index {index} missing");
        }

        // The builder leaves the demo-mode overrides pointing at the fixture,
        // so a consumer's own `Storage::init()` lands on this same database.
        let storage = Storage::init().expect("reopen fixture storage");
        drop(storage);
    }

    // @lat: [[backend#Backend#Database#Retention fixture#Retention Fixture Test Specs#Spec Validation]]
    #[test]
    fn oversized_and_empty_specs_are_rejected() {
        let empty = RetentionFixtureSpec {
            months: 0,
            ..small_spec()
        };
        match build_retention_fixture(&empty).map(|_| ()) {
            Err(RetentionFixtureError::EmptySpec { field }) => assert_eq!("months", field),
            other => panic!("expected EmptySpec, got {other:?}"),
        }

        let oversized = RetentionFixtureSpec {
            owned_rows_per_month: MAX_ROWS_PER_MONTH + 1,
            ..small_spec()
        };
        match build_retention_fixture(&oversized).map(|_| ()) {
            Err(RetentionFixtureError::BucketCapacity { requested, maximum }) => {
                assert_eq!(MAX_ROWS_PER_MONTH + 1, requested);
                assert_eq!(MAX_ROWS_PER_MONTH, maximum);
            }
            other => panic!("expected BucketCapacity, got {other:?}"),
        }
    }
}
