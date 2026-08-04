# VACUUM wall-time and quiesce spike

This spike measures SQLite VACUUM at the 7.45 GB production-size envelope and demonstrates the maintenance-boundary quiesce contract required before compaction ships.

**Archived 2026-08-03.** `vacuum_spike` was removed after this measurement was frozen. References below describe the original run and are not runnable commands.

## Method

The former `src-tauri/src/bin/vacuum_spike.rs` created a 7,450,000,000-byte SQLite WAL fixture with 6.58 GB of retained data and a 0.87 GB `tool_actions_legacy_v30` stand-in. It dropped the stand-in, then timed `VACUUM` on a separately opened maintenance connection.

The same executable prototyped the process-wide quiesce flag: an ingest attempt observed the active flag as retriable, then observed it clear after maintenance completed. It never wrote during the active window, so the later HTTP/backfill boundary could retry rather than lose an ingest event.

## Result

Measured 2026-07-23 with the now-archived `cargo run --release --bin vacuum_spike` invocation from
`src-tauri/` on Linux 6.17.0-29-generic x86_64, 64 logical CPUs, and
rustc 1.95.0. The fixture is temporary and is removed after each run;
production data is never opened by the spike.

| Metric | Value |
| --- | --- |
| Fixture target | 7,450,000,000 bytes |
| File before VACUUM | 7,457,353,728 bytes |
| File after VACUUM | 6,586,490,880 bytes |
| Reclaimed space | 870,862,848 bytes |
| VACUUM wall time | 82,464 ms (82.464 s) |
| Quiesce contract | retry while active; resume after maintenance |

## Decision

The future `compact_database` command must remain user-triggered, preflight roughly twice the database size, use a dedicated connection, and return a retriable boundary while quiesced. This spike does not implement that command or alter application ingest behavior.
