# `EXPLAIN QUERY PLAN` proof for the `session_events` index drop

Gate for Phase 1's `DROP INDEX IF EXISTS
idx_session_events_provider_source`. Verdict: **pass** — after the drop every
`session_events` statement constrained on `(provider, source_key)` still
reports `SEARCH ... USING INDEX uidx_se_owned`. No plan degrades to a `SCAN`.

Reproduce with `cargo run --bin eqp_index_drop_spike` from `src-tauri/`
(`src-tauri/src/bin/eqp_index_drop_spike.rs`). Set `QUILL_EQP_DB` to a real
`usage.db` to dump that database's own `session_events` DDL, confirm it
carries no `ANALYZE` statistics, and rebuild the fixture from the live schema
instead of the vendored copy. The spike only reads plans; it never writes to
the database named by `QUILL_EQP_DB`.

## Query inventory

Grep over `src-tauri/src/` for every `session_events` statement constrained on
`(provider, source_key)` returns exactly the three delete sites the plan named,
all issued through `format!("DELETE FROM {table} ...")` loops over the five
source-owned analytics tables:

| Site | Function | Statement |
| --- | --- | --- |
| storage.rs:2225 | `suppress_transcript_analytics_sources_in_transaction` | `DELETE FROM session_events WHERE provider = ?1 AND source_key = ?2` |
| storage.rs:3339 | `prune_transcript_analytics_sources_for_root` | `DELETE FROM session_events WHERE provider=?1 AND source_key=?2` |
| storage.rs:3457 | `replace_transcript_analytics_snapshot` | `DELETE FROM session_events WHERE provider=?1 AND source_key=?2` |

**No `SELECT` or `UPDATE` anywhere in `src-tauri/src/` constrains
`session_events` on `(provider, source_key)`.** The only other `session_events`
readers are the two `INDEXED BY idx_se_timestamp_chain` runtime selects
(storage.rs:16542/:16547); the remaining sites are inserts (:3492, :15200,
:16375), migration DDL, and tests. Two same-table controls are probed anyway,
because the drop must not perturb them either: the source-less live delete
(storage.rs:2266) and the pinned runtime select (storage.rs:16542).

## Environment

- SQLite `3.45.0`, the vendored `libsqlite3-sys 0.28.0` build behind
  `rusqlite 0.31` with `features = ["bundled"]`.
- Live database `~/.local/share/com.quilltoolkit.app/usage.db` (7.55 GB),
  `sqlite_stat1` rows for `session_events`: **0**. Quill never runs `ANALYZE`,
  so these plans are decided by schema alone and the in-memory replica built
  from the live DDL is faithful.
- The live database's `session_events` DDL matches the migration 30/31/32 plus
  `ensure_startup_indexes` definitions verbatim, including
  `uidx_se_owned(provider, source_key, event_key) WHERE source_key IS NOT
  NULL` and the plain `idx_session_events_provider_source(provider,
  source_key)`.

## Plans

Live database, index present (read-only, no writes):

```
storage.rs:2225  SEARCH session_events USING INDEX idx_session_events_provider_source (provider=? AND source_key=?)
storage.rs:3339  SEARCH session_events USING INDEX idx_session_events_provider_source (provider=? AND source_key=?)
storage.rs:3457  SEARCH session_events USING INDEX idx_session_events_provider_source (provider=? AND source_key=?)
control :2266    SEARCH session_events USING INDEX idx_se_provider_session_sidechain (provider=? AND session_id=?)
control :16542   SEARCH session_events USING COVERING INDEX idx_se_timestamp_chain (timestamp>?)
```

Replica of the live DDL, index dropped:

```
storage.rs:2225  SEARCH session_events USING INDEX uidx_se_owned (provider=? AND source_key=?)
storage.rs:3339  SEARCH session_events USING INDEX uidx_se_owned (provider=? AND source_key=?)
storage.rs:3457  SEARCH session_events USING INDEX uidx_se_owned (provider=? AND source_key=?)
control :2266    SEARCH session_events USING INDEX uidx_se_live (provider=? AND session_id=?)
control :16542   SEARCH session_events USING COVERING INDEX idx_se_timestamp_chain (timestamp>?)
```

`source_key = ?` implies `source_key IS NOT NULL`, and SQLite 3.45.0 makes
that implication: the partial index is usable and is chosen once the plain
prefix index is gone.

## Tie-break note (not a fail)

Which index an *undropped* schema prefers depends on index creation order,
because `idx_session_events_provider_source` and `uidx_se_owned` are equally
ranked for these predicates without `sqlite_stat1`. The replica built from the
live DDL already prefers `uidx_se_owned` before the drop; the vendored schema,
created in migration order, prefers the plain index. The control at :2266
shifts between `idx_se_provider_session_sidechain` and `uidx_se_live` for the
same reason. **Post-drop the result is identical under both orderings**, and no
configuration produced a `SCAN`, so the tie-break is irrelevant to the gate.

## Verdict

Pass. Phase 1 may ship the drop. The permanent `EXPLAIN QUERY PLAN` assertion
test is owned by the drop item, not by this spike.
