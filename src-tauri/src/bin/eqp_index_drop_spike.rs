//! One-off `EXPLAIN QUERY PLAN` proof for the retention index drop.
//!
//! Gates dropping `idx_session_events_provider_source(provider, source_key)`
//! in favour of the partial `uidx_se_owned(provider, source_key, event_key)
//! WHERE source_key IS NOT NULL`. Every `session_events` statement that
//! constrains `(provider, source_key)` must still report a `SEARCH ... USING
//! INDEX uidx_se_owned` once the plain index is gone; a single `SCAN` is a
//! fail and the index stays.
//!
//! This is a spike, deliberately separate from the application path: it only
//! reads plans, never ships behaviour. The permanent regression assertion is
//! owned by the drop itself. Run with `cargo run --bin eqp_index_drop_spike`.
//! Set `QUILL_EQP_DB` to a real `usage.db` to dump that database's own
//! `session_events` DDL, confirm it carries no `ANALYZE` statistics, and
//! rebuild the fixture from the live schema instead of the vendored copy.

use std::{env, error::Error, path::Path};

use rusqlite::{Connection, OpenFlags};

/// `session_events` schema as migrations 30, 31 and 32 plus
/// `ensure_startup_indexes` leave it. Used when no real database is offered.
const VENDORED_SCHEMA: &str = "
CREATE TABLE session_events (
    provider        TEXT NOT NULL,
    source_key      TEXT,
    event_key       TEXT NOT NULL CHECK(length(event_key) > 0),
    session_id      TEXT NOT NULL CHECK(length(session_id) > 0),
    chain_id        TEXT NOT NULL CHECK(length(chain_id) > 0),
    parent_chain_id TEXT,
    agent_id        TEXT,
    is_sidechain    INTEGER NOT NULL DEFAULT 0,
    timestamp       TEXT NOT NULL,
    kind            TEXT NOT NULL,
    uuid            TEXT,
    parent_uuid     TEXT,
    CHECK(source_key IS NULL OR length(source_key) > 0)
);
CREATE UNIQUE INDEX uidx_se_owned
    ON session_events(provider, source_key, event_key)
    WHERE source_key IS NOT NULL;
CREATE UNIQUE INDEX uidx_se_live
    ON session_events(provider, session_id, event_key)
    WHERE source_key IS NULL;
CREATE INDEX idx_session_events_provider_source
    ON session_events(provider, source_key);
CREATE INDEX idx_se_timestamp ON session_events(timestamp);
CREATE INDEX idx_se_chain
    ON session_events(provider, session_id, chain_id, timestamp);
CREATE INDEX idx_se_provider_session_sidechain
    ON session_events(provider, session_id, is_sidechain, timestamp);
CREATE INDEX idx_se_provider_chain_timestamp
    ON session_events(provider, chain_id, timestamp);
CREATE INDEX idx_se_timestamp_chain
    ON session_events(timestamp, provider, chain_id, is_sidechain, kind,
                      session_id);
";

const DROPPED_INDEX: &str = "idx_session_events_provider_source";
const REQUIRED_INDEX: &str = "uidx_se_owned";

/// A statement under proof, verbatim from the site that issues it.
struct Probe {
    site: &'static str,
    sql: &'static str,
    /// `true` when the drop must not cost this statement its index seek.
    gated: bool,
}

/// The three `(provider, source_key)` delete sites the drop has to survive,
/// plus two same-table controls that must not regress either.
const PROBES: [Probe; 5] = [
    Probe {
        site: "storage.rs:2225 suppress_transcript_analytics_sources_in_transaction",
        sql: "DELETE FROM session_events WHERE provider = ?1 AND source_key = ?2",
        gated: true,
    },
    Probe {
        site: "storage.rs:3339 prune_transcript_analytics_sources_for_root",
        sql: "DELETE FROM session_events WHERE provider=?1 AND source_key=?2",
        gated: true,
    },
    Probe {
        site: "storage.rs:3457 replace_transcript_analytics_snapshot",
        sql: "DELETE FROM session_events WHERE provider=?1 AND source_key=?2",
        gated: true,
    },
    Probe {
        site: "storage.rs:2266 delete_live_analytics_sessions_in_transaction (control)",
        sql: "DELETE FROM session_events \
              WHERE provider = ?1 AND session_id = ?2 AND source_key IS NULL",
        gated: false,
    },
    Probe {
        site: "storage.rs:16542 get_llm_runtime_stats (control)",
        sql: "SELECT timestamp, provider, chain_id, is_sidechain, kind, session_id \
              FROM session_events INDEXED BY idx_se_timestamp_chain \
              WHERE timestamp >= ?1",
        gated: false,
    },
];

/// Collect the `detail` column of every `EXPLAIN QUERY PLAN` row for `sql`.
///
/// The plan is fixed at prepare time, so the probes' bind parameters are
/// filled with `NULL` purely to satisfy the binding arity.
fn plan_of(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
    let unbound = vec![rusqlite::types::Null; statement.parameter_count()];
    let rows = statement.query_map(rusqlite::params_from_iter(unbound), |row| {
        row.get::<_, String>(3)
    })?;
    rows.collect()
}

/// Report each probe's plan and return the gated probes that failed to seek
/// through the partial index. A new vector is built; nothing is mutated.
fn report(conn: &Connection, phase: &str) -> rusqlite::Result<Vec<String>> {
    println!("--- {phase} ---");
    let mut failures = Vec::new();
    for probe in &PROBES {
        let plan = plan_of(conn, probe.sql)?;
        let joined = plan.join(" | ");
        println!("{}\n    plan: {joined}", probe.site);
        if probe.gated && !joined.contains(&format!("USING INDEX {REQUIRED_INDEX}")) {
            failures.push(format!("{} -> {joined}", probe.site));
        }
    }
    Ok(failures)
}

/// Read the `session_events` schema a real database actually carries, and
/// report whether `ANALYZE` statistics could be steering its planner.
fn schema_from_live_db(path: &Path) -> Result<String, Box<dyn Error>> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;

    let stat_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'sqlite_stat1'",
            [],
            |row| row.get(0),
        )
        .and_then(|present: i64| {
            if present == 0 {
                Ok(0)
            } else {
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_stat1 WHERE tbl = 'session_events'",
                    [],
                    |row| row.get(0),
                )
            }
        })?;
    println!("live_db={}", path.display());
    println!("live_db_sqlite_stat1_rows_for_session_events={stat_rows}");

    let mut statement = conn.prepare(
        "SELECT sql FROM sqlite_master
         WHERE tbl_name = 'session_events' AND sql IS NOT NULL
         ORDER BY type DESC, name",
    )?;
    let statements: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if statements.is_empty() {
        return Err("live database has no session_events table".into());
    }
    for ddl in &statements {
        println!("live_ddl: {}", ddl.replace('\n', " "));
    }

    println!();
    let off_target = report(&conn, "live database, index present")?;
    println!(
        "note: {} gated site(s) still prefer {DROPPED_INDEX} while it exists",
        off_target.len()
    );
    println!();

    Ok(statements
        .iter()
        .map(|ddl| format!("{ddl};\n"))
        .collect::<String>())
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("sqlite_version={}", rusqlite::version());

    let schema = match env::var_os("QUILL_EQP_DB") {
        Some(path) => schema_from_live_db(Path::new(&path))?,
        None => {
            println!("live_db=<none> (set QUILL_EQP_DB to prove against a real usage.db)");
            println!();
            VENDORED_SCHEMA.to_string()
        }
    };

    let fixture = Connection::open_in_memory()?;
    fixture.execute_batch(&schema)?;
    let off_target = report(&fixture, "fixture, index present")?;
    println!(
        "note: {} gated site(s) still prefer {DROPPED_INDEX} while it exists",
        off_target.len()
    );

    println!();
    fixture.execute_batch(&format!("DROP INDEX IF EXISTS {DROPPED_INDEX};"))?;
    let after = report(&fixture, "fixture, index dropped")?;

    println!();
    if after.is_empty() {
        println!("verdict=pass");
        println!("every (provider, source_key) site still searches {REQUIRED_INDEX}");
        Ok(())
    } else {
        println!("verdict=fail");
        for failure in &after {
            println!("regressed: {failure}");
        }
        Err(format!("{} gated site(s) lost the index seek", after.len()).into())
    }
}
