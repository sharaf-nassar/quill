---
title: A fixed-date fixture ages out of a wall-clock range window
date: 2026-09-24
component: transcript-analytics-tests
tags: [tests, fixtures, query-clock, range-window, ci]
problem_type: convention
---

# A fixed-date fixture ages out of a wall-clock range window

## Problem

The v0.5.0 CI run on 2026-09-24 failed in
`transcript_analytics::tests::pi_summary_usage_reconciles_without_turn_inflation`
with `left: 0, right: 2032`. The same test passed in the v0.4.5 run on
2026-09-22, and nothing between the two tags touched the test, its fixture, or
the queries it reads.

## Root cause

`src-tauri/src/fixtures/pi-usage-evidence.jsonl` is dated 2026-08-24T05:00Z,
but the test read `get_token_stats("30d")`, the 30-day model overview, and the
30-day session history against the wall clock. After 2026-09-23T05:00Z the rows
fell outside the window: the raw-table assertions still passed, and the first
range read returned 0 tokens. `get_session_model_history` also read
`Utc::now()` directly, so it ignored the test clock override.

## Fix

Wrap each range read in `storage::with_pinned_query_now` with a clock one hour
after the fixture, and make `get_session_model_history` take its range end from
`query_now()`, as the overview and `range_from_timestamp` already do.
Production behavior is unchanged because only tests pin the clock. The full
Rust suite then passed (630 passed, 12 ignored).

## Prevention

- A test that seeds fixed dates and reads a sliding range must pin the query
  clock, or stamp its rows relative to the test clock as the migration-36
  fixture does.
- Range readers take "now" from `query_now()`, never `Utc::now()`, so tests can
  pin them.
- Raw rows present but range totals at zero points at the window, not the
  ingest path.
