# Footprint and cost measurement for the `session_events` index drop

Observational record for Phase 1's `DROP INDEX IF EXISTS
idx_session_events_provider_source`. These numbers are corpus-dependent and
are **not** pass/fail thresholds — they exist so the reclaim is known to be
worth it and the startup cost every user pays once is known before it ships.
The pass/fail gate is [`eqp-index-drop-proof.md`](./eqp-index-drop-proof.md);
the permanent regression assertions live in `storage.rs`'s test module.

**Archived 2026-08-03.** `index_drop_measure_spike` was removed after this measurement was frozen. Its original, no-longer-runnable invocation was
`QUILL_INDEX_DROP_DB=~/.local/share/com.quilltoolkit.app/usage.db cargo run
--release --bin index_drop_measure_spike` from `src-tauri/`
(`src-tauri/src/bin/index_drop_measure_spike.rs`).

## Method

The source database is opened **read-only** and copied with `VACUUM INTO`, so
the running app may keep writing throughout — a plain file copy of a live
7.5 GB WAL database would be torn. The copy is consequently already compacted,
which is deliberate: `bytes_before → bytes_after` is then the index's own
footprint rather than that footprint plus whatever unrelated free pages the
source happened to be carrying. It is the stricter number.

The copy is put in WAL mode with `synchronous = NORMAL` and its WAL truncated
to zero before the drop, so `wal_bytes_after_drop` is the drop's own WAL
production and nothing else. `VACUUM` then runs on a dedicated connection with
a 5 s busy timeout, mirroring `Storage::vacuum_database`.

## Environment

- SQLite 3.45.0, the vendored `libsqlite3-sys 0.28.0` build behind
  `rusqlite 0.31` with `features = ["bundled"]`. Release profile.
- Source `~/.local/share/com.quilltoolkit.app/usage.db`, 7,552,692,224 bytes
  (7.55 GB), with the Quill app running and writing concurrently.
- Linux, NVMe root filesystem.

## Results

| Measurement | Value |
| --- | --- |
| Source file | 7,552,692,224 B (7.55 GB) |
| `VACUUM INTO` copy wall time | 65.8 s |
| Copy before the drop (`bytes_before`) | 7,535,165,440 B (7.54 GB) |
| WAL before the drop | 0 B (truncated) |
| **`DROP INDEX` wall time** | **416 ms** |
| **WAL the drop itself produced** | **482,072 B (471 KiB)** |
| File bytes after the drop, before VACUUM | 7,535,165,440 B — **unchanged** |
| `VACUUM` wall time | 168.0 s |
| File bytes after VACUUM (`bytes_after`) | 6,808,276,992 B (6.81 GB) |
| **Whole-file bytes reclaimed** | **726,888,448 B (693 MiB / 727 MB)** |

## Reading

**The drop is cheap at startup.** 416 ms on a 7.5 GB database, producing
471 KiB of WAL. `DROP INDEX` only walks the index's page tree onto the
freelist; it neither rewrites the table nor touches the other seven
`session_events` indexes, so the one-time first-open cost is a fraction of a
second and the WAL it dirties is four orders of magnitude below the ~727 MB it
releases. That answers the concern that motivated measuring it: this runs on
the UI thread's path to a usable app, and it is not a stall a user would
notice.

**Deletion frees no filesystem bytes.** `bytes_after_drop_before_vacuum`
equals `bytes_before` exactly. The reclaim only materialises through a
`compact_database` run — the same S2 caveat Phase 2's consent copy has to
state, demonstrated here on the non-destructive half of the feature.

**The reclaim exceeds the estimate.** The spec's inventory put the index at
472.7 MB; the measured reclaim is 726.9 MB. The corpus has grown since that
inventory was taken, and the measurement here is whole-file page bytes rather
than an inventory estimate. The direction is favourable, so this is recorded
rather than reconciled — Phase 1's non-destructive target is met and then
some.

**VACUUM dominates.** 168 s of compaction against 0.4 s of dropping. Whether
the reclaim is realised is a question about the existing compaction path's
ergonomics, not about the drop, which is exactly why the drop ships
unconditionally at startup and the compaction stays user-triggered.
