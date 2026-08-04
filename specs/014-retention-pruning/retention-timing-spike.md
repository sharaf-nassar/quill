# Retention timing spike — measured budgets

Captured output of the former `src-tauri/src/bin/retention_spike.rs`, the measurement the plan
defers every retention numeric budget to ("*Budgets come from a spike, not
from this document*", plan.md). This file records what the spike measured and
the constants the chunked delete engine and its preflight are therefore
allowed to hard-code. It is a **measurement, not a test** — nothing here is a
CI threshold.

**Archived 2026-08-03.** `retention_spike` was removed after these budgets were frozen. Its original invocation, `cargo run --release --bin retention_spike` from `src-tauri/`, is no longer runnable.
The former `QUILL_RETENTION_SPIKE_*` environment variables could shrink the corpus for a
smoke run; every number below comes from the defaults.

## Environment

- AMD Ryzen Threadripper 3970X, 125 GB RAM, NVMe root filesystem, Linux 6.17.
- SQLite 3.45.0 — the vendored `libsqlite3-sys 0.28.0` build behind
  `rusqlite 0.31` with `features = ["bundled"]`.
- Release profile. Every published constant reproduced across repeated full
  runs to within a few percent, and every conclusion — chunk size,
  `temp_store`, the Counting-phase verdict — was identical in all of them.
- Measured **after** Phase 1's `idx_session_events_provider_source` drop
  landed, since the fixture builds its schema through `Storage::init`. That
  drop is visible in these numbers: it took 23 MB off the corpus file and
  about 6% off the WAL every deleted row produces, because it is one fewer
  index to rewrite per row.

## Corpus

Built by the shared `build_retention_fixture` — the same `pub` builder the
Phase 2 tests consume, which is the point of the fixture bead.

| | |
| --- | --- |
| Buckets (30-day "months") | 24 |
| Source-owned conforming rows per bucket per table | 16,700 |
| Live (`source_key IS NULL`) rows per bucket per table | 200 |
| Rows total, five analytics tables | 2,028,360 |
| Database file | 1,289,674,752 B (1.29 GB) |
| Months retained by the measured cutoff | 3 |
| **Doomed rows** (`tool_actions` 350,700 + `session_events` 350,700) | **701,400** |
| Build wall time | ~29 s |

## Published budgets

These are the constants the delete engine and preflight take from this spike.

| Budget | Value | Derivation |
| --- | --- | --- |
| Chunk size | **25,000 rows** | Largest swept size whose pooled p95 transaction hold stays under 1,000 ms |
| Per-chunk wall target | **1,603 ms** | measured p95 hold 534.3 ms × 3 headroom |
| WAL bytes per row | **788.7** | worst full chunk's WAL ÷ its rows, at the chosen chunk size |
| TEMP bytes per doomed row | **11.05** | temp b-tree bytes ÷ doomed rows; identical under both `temp_store` settings |
| Delete-phase preflight, WAL term | **19,718,352 B** | 788.7 × 25,000, one chunk |
| Delete-phase preflight, TEMP term | **7,753,728 B** | 11.05 × 701,400, both doomed tables |
| Free-space re-check interval `N` | **3 chunks** | ≈1 s of work per re-check at the measured mean hold of 417.7 ms |
| Counting-phase budget | **2,616 ms** | measured 871.7 ms × 3 headroom |
| Stale-preview tolerance | **2,616 ms** | one Counting phase — past this the preview costs more to trust than to redo |
| Total wall-time budget | **40,598 ms** | measured 13,533 ms × 3 headroom |

The safety multiplier the plan asks the preflight to add sits **on top** of
the two preflight terms; it is the engine's choice, not this spike's. The
headroom multiplier of 3 above is what turns a measurement on this machine
into a ceiling a slower machine must still fit under.

### Why 1,000 ms is the chunk-hold ceiling

The chunk hold is the interval between progress emissions and the latency to
notice an abort condition. The plan's requirement is that the phase *visibly
advances* — not that each step feels instantaneous. One update per second
satisfies that, and finer granularity is bought with real total wall time,
because a smaller chunk amortizes each chunk's fixed cost over fewer rows. At
5,000 rows the run takes 29.5 s to do exactly the work 25,000 rows does in
13.5 s.

## Chunk-size sweep

Each size ran against its own fresh copy of the corpus, so none of them
inherits another's cache state. All five deleted exactly 701,400 rows.

| Chunk | Chunks | Counting (ms) | Delete (ms) | Total (ms) | Hold mean | Hold p95 | Hold max | WAL max (B) | WAL/row |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 5,000 | 142 | 867.6 | 28,598.5 | 29,466.1 | 194.0 | 333.2 | 744.9 | 4,997,592 | 999.5 |
| 10,000 | 72 | 885.3 | 19,092.3 | 19,977.6 | 263.1 | 403.7 | 1,680.3 | 8,783,872 | 878.4 |
| **25,000** | **30** | **871.7** | **12,660.8** | **13,532.6** | **417.7** | **534.3** | **1,830.5** | **19,718,352** | **788.7** |
| 50,000 | 16 | 848.3 | 10,378.9 | 11,227.2 | 641.2 | 2,172.7 | 2,172.7 | 35,827,552 | 716.6 |
| 100,000 | 8 | 862.1 | 9,749.5 | 10,611.6 | 1,208.4 | 3,063.7 | 3,063.7 | 69,545,632 | 695.5 |

Reading the curve:

- **Total wall time flattens after 25,000.** Going from 25,000 to 100,000
  buys 2.9 s (22%) and costs 2.9× the per-chunk hold and 3.5× the WAL
  footprint the preflight must reserve. 25,000 is where the curve turns.
- **WAL per row converges on ~696 B.** A chunk's WAL is a fixed page overhead
  plus a per-row term; the per-row figure only falls as the fixed part
  amortizes. The published 788.7 is the rate at the chosen chunk size, which
  is what the preflight multiplies by a whole chunk. Deleting one row rewrites
  its entry in every surviving `session_events` index, which is where the bulk
  of those bytes go.
- **`hold max` is a cold-start artifact, not a chunk-size effect.** Every size
  shows one ~0.7–3.1 s chunk — the first `tool_actions` chunk, paying to fault
  in index pages. It does not scale with chunk size, so p95 rather than max is
  the statistic the recommendation rests on.
- **`wal_checkpoint(TRUNCATE)` after every chunk works exactly as designed.**
  Post-checkpoint WAL is **0 bytes** at every chunk size, and the mean
  checkpoint costs 2–10 ms. WAL is bounded by one chunk, never by the run.
- **Deletes do not shrink the file.** `db_bytes_after == db_bytes_before` at
  every chunk size — 1,289,674,752 B either way. Space is reclaimed only by
  the VACUUM that follows, which is exactly what the `"partial"` result's
  "run Compact database" copy has to tell the user.

## `temp_store`: pin `MEMORY`

Both settings materialize the same 7,753,728 B of doomed-rowid b-tree
(11.05 B per doomed row, against an 8-byte rowid — so ~38% b-tree overhead).
The only thing that changes is where those bytes land.

| `temp_store` | Counting total | Temp b-tree | Resident-memory delta | Bytes in unlinked temp files |
| --- | ---: | ---: | ---: | ---: |
| `MEMORY` | 889.6 ms | 7,753,728 | +9,084,928 | 0 |
| `FILE` | 911.4 ms | 7,753,728 | 0 | 7,753,728 |

**Recommendation: pin `PRAGMA temp_store = MEMORY` on the maintenance
connection.** There is no wall-time difference to trade (2%), so the choice
is purely about which resource pays. Under 10 MB of RSS for a 701k-row prune is
negligible for a desktop app, whereas `FILE` puts the term on a temp
filesystem that may not be the one the preflight measured — the plan flags
exactly this ("which may not even be the same filesystem as the database").
`MEMORY` moves the TEMP term out of the disk preflight and into a memory
budget that scales at 11.05 B per doomed row.

The `FILE` measurement had to be taken through `/proc/self/fd`: SQLite creates
its spill file and unlinks it immediately, so it never appears in a directory
listing.

## Scan cost, with and without `idx_se_timestamp`

Measured on two copies of the corpus, one with `idx_se_timestamp` and one
with it dropped. Both copies are warmed first and the repetitions alternate
between them — measuring all repetitions on one copy and then all on the other
hands the second copy a hotter page cache and turns residency into what looks
like an index effect.

`tool_actions` is the control: dropping a `session_events` index cannot change
its plan, so whatever ratio it shows is measurement bias.

| Table | With index | Without index | Raw ratio | Control-normalized |
| --- | ---: | ---: | ---: | ---: |
| `tool_actions` (control) | 1,046.5 ms | 675.8 ms | 1.55 | 1.00 |
| `session_events` | 821.9 ms | 224.4 ms | 3.66 | **2.37** |

Query plans:

| Table | With `idx_se_timestamp` | Without |
| --- | --- | --- |
| `tool_actions` | `SCAN tool_actions USING INDEX uidx_ta_owned` | *(unchanged)* |
| `session_events` | `SEARCH session_events USING INDEX idx_se_timestamp (timestamp<?)` | `SEARCH session_events USING INDEX idx_se_timestamp_chain (timestamp<?)` |

Two findings:

1. **`tool_actions` does not table-scan.** The plan's premise — "`tool_actions`
   has no index leading with `timestamp`, so its pass is a full table scan
   every time" — is right about the absence of the index but wrong about the
   consequence: the planner walks the partial unique index `uidx_ta_owned`
   instead, which already encodes `source_key IS NOT NULL`. It is still a full
   index scan and still the most expensive single statement in the Counting
   phase (~693 ms of the ~890 ms), so the design conclusion is unchanged. The
   description in plan.md should be corrected when the engine lands.

2. **`idx_se_timestamp` makes the Counting scan 2.37× *slower*.** With it
   present the planner picks it; with it dropped the planner falls back to
   `idx_se_timestamp_chain(timestamp, provider, chain_id, is_sidechain, kind,
   session_id)` and the same scan runs in a quarter of the time. This is a
   measured result whose mechanism this spike does not establish, and it is
   **out of scope to act on here** — `idx_se_timestamp` serves readers this
   spike never exercised. It is recorded as a signal for whoever revisits that
   index; the retention engine must not drop it on this evidence alone.

## Free-space re-check

A `statvfs` call costs **3.16 µs**. At the recommended `N = 3` chunks the
re-check consumes 2.5 × 10⁻⁶ of the window it guards — comfortably the "noise
against chunk wall time" the plan requires, with room to shorten `N` if the
engine wants a tighter abort latency.

## Design signal: the Counting scan does not dominate

The question the plan reserved for this spike: the design pays for the scan
twice, once in `preview_retention` and again in `run_retention_maintenance`,
which deliberately rescans under its own lease. If the scan dominated the run,
that would reopen whether the preview should take the lease and hand the run
its materialized doomed set.

At the recommended chunk size:

| | |
| --- | --- |
| Counting phase | 871.7 ms |
| Delete phase | 12,660.8 ms |
| **Counting share of run** | **6.4%** |

**Verdict: keep the two-scan / two-lease split.** The second scan costs 872 ms
against a 13.5 s run. Collapsing to one scan would save ~6% of the run in
exchange for holding the maintenance lease across the user's confirmation
dialog — an unbounded wait during which all ingest is quiesced. That trade is
plainly bad, and the current architecture stands. The margin is wide enough
(a factor of 15) that this conclusion is not sensitive to machine speed.

One consequence for the UI does survive: at ~870 ms the Counting phase is long
enough to look like a hang without the progress heartbeat the plan specifies,
and `preview_retention` is *nothing but* that phase. The heartbeat is load-
bearing, not decoration.
