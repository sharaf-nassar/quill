//! Retention timing spike (feature 014).
//!
//! This binary exists to be the *second* consumer of the frozen synthetic
//! retention corpus. The fixture's whole value is that acceptance tests and
//! budget measurements run on one corpus, and the only way to keep that true
//! is for the spike to link the same `pub` builder the tests use — which this
//! file proves it can, rather than leaving it assumed.
//!
//! At this stage it builds the corpus, reports the exact per-month counts, and
//! checks the plan against the database. The delete-timing, WAL-bytes and
//! TEMP-table measurements that fix the numeric budgets are layered on top of
//! this same corpus by the timing-spike work item.
//!
//! Run with: `cargo run --bin retention_spike`

use quill_lib::retention_fixture::{
    RetentionFixtureSpec, RetentionRowKind, RetentionTable, build_retention_fixture, count_rows,
};

/// Corpus size for the spike. Larger than the unit-test spec so wall-time and
/// byte measurements have something to measure, small enough to build quickly.
const SPIKE_MONTHS: u32 = 12;
const SPIKE_OWNED_ROWS_PER_MONTH: u32 = 2_000;
const SPIKE_LIVE_ROWS_PER_MONTH: u32 = 200;
const SPIKE_SOURCES: u32 = 24;

/// Months of history the reported cutoff retains.
const SPIKE_MONTHS_RETAINED: u32 = 3;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spec = RetentionFixtureSpec {
        months: SPIKE_MONTHS,
        owned_rows_per_month: SPIKE_OWNED_ROWS_PER_MONTH,
        live_rows_per_month: SPIKE_LIVE_ROWS_PER_MONTH,
        sources: SPIKE_SOURCES,
        ..RetentionFixtureSpec::default()
    };

    let build_started = std::time::Instant::now();
    let fixture = build_retention_fixture(&spec)?;
    let build_wall_time = build_started.elapsed();

    let plan = fixture.plan();
    let conn = fixture.open_connection()?;
    let cutoff = plan.boundary_timestamp(SPIKE_MONTHS_RETAINED);

    println!("db_path={}", fixture.db_path().display());
    println!("build_wall_time_ms={}", build_wall_time.as_millis());
    println!("anchor={}", plan.boundary_timestamp(0));
    println!("months={}", plan.months());
    println!("sources={}", plan.sources());
    println!("months_retained={SPIKE_MONTHS_RETAINED}");
    println!("cutoff={cutoff}");

    for table in RetentionTable::ALL {
        for kind in RetentionRowKind::ALL {
            let planned = plan.total_rows(table, kind);
            let stored = count_rows(&conn, table, kind)?;
            // The spike's numbers are only comparable to the tests' numbers if
            // both ran on the same corpus, so a drift here is fatal, not a
            // warning to print past.
            assert_eq!(
                planned,
                stored,
                "planned/stored row count drift for {} / {kind:?}",
                table.as_str()
            );
            println!("rows.{}.{kind:?}={stored}", table.as_str());
        }
        println!(
            "doomed.{}={}",
            table.as_str(),
            plan.rows_before_boundary(
                SPIKE_MONTHS_RETAINED,
                table,
                RetentionRowKind::OwnedConforming
            )
        );
    }

    let file_bytes = std::fs::metadata(fixture.db_path())?.len();
    println!("db_bytes={file_bytes}");
    Ok(())
}
