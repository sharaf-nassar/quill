# VACUUM wall-time and quiesce spike

This spike measures SQLite VACUUM at the 7.45 GB production-size envelope and demonstrates the maintenance-boundary quiesce contract required before compaction ships.

## Method

[[src-tauri/src/bin/vacuum_spike.rs]] creates a 7,450,000,000-byte SQLite WAL fixture with 6.58 GB of retained data and a 0.87 GB `tool_actions_legacy_v30` stand-in. It drops the stand-in, then times `VACUUM` on a separately opened maintenance connection.

The same executable prototypes the process-wide quiesce flag: an ingest attempt observes the active flag as retriable, then observes it clear after maintenance completes. It never writes during the active window, so the later HTTP/backfill boundary can retry rather than lose an ingest event.

## Result

Measured 2026-07-23 with `cargo run --release --bin vacuum_spike` from
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
