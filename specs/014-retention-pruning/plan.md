# Plan: retention-pruning

**Historical implementation plan.** The spike binaries named below were removed on 2026-08-03 after their evidence was frozen. Their paths and commands document completed design work and are not current runnable surfaces.

Implementation plan for the clarified spec. All seven Clarifications (Q1–Q7)
are binding, as are the Spec Review's code-verified corrections. Scope is one
epic in two phases: **Phase 1 non-destructive** (drop
`idx_session_events_provider_source` behind an `EXPLAIN QUERY PLAN` proof;
settle the `full_input`/`full_output` write policy for `tool_detail`), then
**Phase 2 destructive** (opt-in, age-based retention over `tool_actions` and
`session_events`, enforced at insert time and handed off to the existing
compaction path).

Grounding facts verified against this worktree:

- `src-tauri/src/lib.rs`: `MAINTENANCE_IN_PROGRESS: AtomicBool` (:67),
  `IngestQuiesceGuard` (:90), `begin_ingest_quiesce()` (:98),
  `ingest_is_quiesced()` (:110), `with_ingest_write_permit()` (:120).
  `begin_ingest_quiesce()` is a bare `parking_lot` `RwLock::write()` on the
  process-wide `INGEST_GATE` — it **blocks unboundedly** and has no `try_`
  variant today, so two maintenance operations issued concurrently do not
  error, they queue.
  `compact_database` (:3407) is `spawn_blocking` → `begin_ingest_quiesce()`
  → `preflight_database_compaction()` → `vacuum_database()`, emitting
  `compact-database-progress` via `DatabaseCompactionProgress { phase:
  &'static str, pct: u8 }` (:3386) and `compact-database-finished` (:3426).
  The quiesce invariant test is `write_arriving_during_quiesce_lands_after_
  unquiesce` (:4964). Backend-only settings keys already follow a dotted
  convention — `transcript_rescan.enabled` / `.interval_seconds` (:176–177)
  — read with `read_bool_setting` (:3243) / `read_i64_setting` (:3252) and
  deliberately kept out of `RuntimeSettings`.
- `src-tauri/src/storage.rs`: `Storage { conn: Mutex<Connection>, db_path,
  + 5 analytics caches }` (:2972). `DatabaseCompactionResult { status:
  &'static str, reason: Option<String>, bytes_before: u64, bytes_after: u64 }`
  (:2988) with `skipped_database_compaction()` (:2994).
  `preflight_database_compaction` (:5912) requires `2 ×` file size free via
  `available_disk_space` (:3006, `statvfs`, Unix-only).
  `vacuum_database` (:5942) opens a **dedicated** `Connection` with a 5 s
  busy timeout and runs `VACUUM;`. `settings` table is `(key TEXT PRIMARY
  KEY, value TEXT NOT NULL)` (:3746); `get_setting` (:11037) / `set_setting`
  (:11046) / `delete_setting` (:11056).
- **Only one writer inserts source-owned rows.**
  `replace_transcript_analytics_snapshot` (:3356) deletes all five owned
  tables for a source (:3457) then reinserts — `session_events` at :3492,
  `tool_actions` at :3551. Every other insert site hard-codes `source_key
  NULL`: `store_live_session_analytics` (:15200) and `ingest_session_events`
  (:16375). `server.rs::persist_remote_session_analytics` (:855) delegates to
  `store_live_session_analytics` (:888). **`server.rs` therefore needs no
  watermark filtering** — its rows are live rows, which are excluded from
  retention by Q3.
- `replace_transcript_analytics_snapshot` already reads one `settings` row
  inside its transaction (the generation key, :3380), so a watermark read
  piggybacks on an existing pattern for the cost of one PK lookup.
- Three delete sites depend on `(provider, source_key)` over
  `session_events`, not one: `suppress_transcript_analytics_sources_in_
  transaction` (:2225), `prune_transcript_analytics_sources_for_root`
  (:3339), and the snapshot replace (:3457). The `EXPLAIN QUERY PLAN` proof
  must cover all three.
- Index definitions live in the migration-30 block:
  `idx_session_events_provider_source ON session_events(provider,
  source_key)` (:5481) is a **plain** index; the partial candidate is
  `uidx_se_owned(provider, source_key, event_key) WHERE source_key IS NOT
  NULL`. `ensure_startup_indexes` (:1493) recreates `idx_se_timestamp_chain`
  on every open (:1548) because `get_llm_runtime_stats` pins it with
  `INDEXED BY`; it does **not** create `idx_session_events_provider_source`,
  which is why the drop can live there without a migration.
- Payload readers: `full_input` / `full_output` are read in SQL by exactly
  three commands — `get_code_stats` (:15869/:15872), `get_code_stats_history`
  (:15976/:15978), `get_batch_session_code_stats` (:16077/:16081) — and all
  three are gated on `category = 'code_change'`. No SQL reader touches
  `full_input` for `tool_detail`; no frontend file references `full_input` or
  `full_output` at all. `tool_detail` is produced in `sessions.rs:2240/2248`,
  and Tantivy's `tool_details` field is fed from parsed messages
  (`sessions.rs:1109`), not from `tool_actions`.
- Range readers cap at 30 days. `range_to_duration` (:1230) knows only
  `1h/24h/7d/30d` and falls through to 24 h — there is no `all` arm — so
  `get_code_stats`, `get_code_stats_history`, and `get_llm_runtime_stats`
  never ask for data older than 30 days. `get_batch_session_code_stats`,
  `get_session_breakdown` (:10628), and `get_session_subagent_tree` (:10776)
  are session-scoped and therefore the only readers a 90-day policy can
  starve. On the frontend the same cap is typed: `RangeType` is
  `"1h" | "24h" | "7d" | "30d"` (src/types.ts:212) — **there is no `all`
  member anywhere in the range vocabulary**. The only "All time" affordances
  in the product are the two Breakdown toggles
  (`BreakdownPanel.tsx:967` skills, `:1007` hooks), which read `skill_usages`
  and `hook_invocations` — tables retention never prunes.
- 013's cache blast radius is confirmed zero: the declared `CacheTable` sets
  are `MODEL_ANALYTICS_CACHE_TABLES` (:182), `MODEL_HISTORY_CACHE_TABLES`
  (:191), `CacheTable::RowId("usage_snapshots")` (:9462), and
  `CacheTable::RowId("context_savings_events")` (:9978). Neither
  `tool_actions` nor `session_events` appears. `TableVersions` (:141) probes
  max-only markers that a DELETE never moves; `ANALYTICS_CACHE_TTL` is 45 s
  (:81).
- Frontend: `src/components/settings/PerformanceTab.tsx` owns the
  "Database maintenance" section header (:173) and the Compact control
  (:174–187), listening to both compaction events (:47–65) with local
  `CompactDatabaseProgress` / `CompactDatabaseResult` types declared inline
  (:14–24). `src/mocks/ipcFixtures.ts:2294` stubs `compact_database` and its
  events. `RuntimeSettings` (`src/hooks/useRuntimeSettings.ts:6`) is saved
  **wholesale** by `PerformanceTab.update()` (`{...settings, ...patch}`).
- `src-tauri/src/bin/vacuum_spike.rs` (105 lines) is the spike pattern:
  a `tempfile::tempdir()` synthetic database built to a target byte size, a
  prototype `QuiesceFlag`, `Instant` timing, and printed measurements.
- The storage test harness pattern is fixed: `mod tests` uses
  `serial_test::serial` (storage.rs:16869) and `init_storage_in` sets
  `QUILL_DEMO_MODE` + `QUILL_DATA_DIR` to a `TempDir` before `Storage::init`
  (:16878), under `#[serial]` because the env block is process-global. Any
  shared fixture builder must follow the same contract or tests race.
- `PRAGMA temp_store` is set to `MEMORY` only on the model-analytics reader
  connection (storage.rs:5903); the primary and `vacuum_database` connections
  inherit the build default. Retention's doomed-rowid `TEMP TABLE`s therefore
  land wherever the default puts them unless the maintenance connection pins
  the pragma explicitly.
- `learning.rs` does **not** read `tool_actions`: its corpus comes from
  `crate::sessions::extract_messages_from_jsonl` over transcript files
  (learning.rs:633). Retention has no learning-pipeline stakeholder today.

---

## Architecture Approach

Two phases inside one epic, ordered so the reclaim that costs no history
lands first and is measured on its own (Q1).

**Phase 1 — non-destructive footprint reduction.**

*Index drop.* `idx_session_events_provider_source(provider, source_key)`
(storage.rs:5481) is ~473 MB on the measured corpus and is a strict prefix
of the partial `uidx_se_owned(provider, source_key, event_key) WHERE
source_key IS NOT NULL`. Every query that could want it constrains
`source_key = ?`, which implies `source_key IS NOT NULL` and therefore makes
the partial index usable. That implication is SQLite-version-dependent
behaviour, so it is **proved, not assumed**: a spike runs `EXPLAIN QUERY
PLAN` against the vendored `libsqlite3-sys 0.28.0` build for all three
`(provider, source_key)` delete sites (storage.rs:2225, :3339, :3457) plus
any `SELECT` the grep turns up, before and after a simulated drop. Pass
means every plan still reports a `SEARCH … USING INDEX uidx_se_owned`; a
single `SCAN` is a fail and the index stays. **If the proof fails, Phase 1
loses ~473 MB of its target — that is a finding to report, not a decision to
reopen.**

The drop itself goes into `ensure_startup_indexes` (storage.rs:1493) as
`DROP INDEX IF EXISTS idx_session_events_provider_source;`, **not** into a
migration. `ensure_startup_indexes` runs on every open, is idempotent, and
never creates that index — only migration 30 does, and migration 30 only
runs on databases below v30. So the drop needs no `v35`, causes no
`SCHEMA_TOO_NEW` lockout, and an older build that reopens the file simply
finds one fewer index. This deliberately mirrors the Q2 posture for the
watermark. `idx_se_timestamp_chain` is untouched — `ensure_startup_indexes`
recreates it on every open precisely because `get_llm_runtime_stats` pins it
with `INDEXED BY` and Quill never runs `ANALYZE`; **retention drops exactly
one index and no other**. `ensure_startup_indexes` acquires a second
responsibility here — its name now says only half of what it does, so the
drop lands with a one-line doc comment on the function recording that it
also drops `idx_session_events_provider_source` and why the drop lives there
rather than in a migration.

As with `DROP TABLE`, the drop frees zero filesystem bytes until a
`compact_database` run, so the headline Phase 1 measurement is index-drop →
compact → whole-file delta. That single number is not sufficient on its own:
`DROP INDEX` on a ~473 MB index runs at startup, on the UI thread's path to
a usable app, and it dirties WAL. The item therefore also records, on a
production-sized copy, the **`DROP INDEX` wall time** (this is a one-time
first-open cost every user pays, and it must be known before it ships) and
the **WAL delta the drop itself produces** (the drop precedes any preflight
and has no disk budget of its own). Whole-file delta after VACUUM answers
"was it worth it"; these two answer "what does it cost the user on the open
where it happens".

*Payload write policy.* The prior evidence is already strong: `full_input` /
`full_output` are read in SQL only by the three code-stats commands, all
gated on `category = 'code_change'`, and no frontend file references either
column. The decision item's job is to close the grep (including
`sessions.rs`, the Tantivy indexer, and the MCP tools) and then commit to a
**forward-only** write policy for `tool_detail` rows — either omit the two
payload columns or stop writing the row. Retroactive NULLing stays a
Non-Goal: `full_input IS NOT NULL` is load-bearing at storage.rs:15872,
:15978, and :16081, and `get_code_stats` reads `full_input` as a
non-optional `String`.

**Phase 2 — opt-in, age-based retention.**

*Durability is an insert-time watermark, not a DELETE.* Resurrection is a
normal-path behaviour: `replace_transcript_analytics_snapshot`
unconditionally deletes and reinserts a source's whole parse whenever
mtime/size/hash changes, so one `--resume` append to a months-old JSONL
restores that source's entire pre-cutoff history. The fix is a global
retention watermark in the `settings` table that the snapshot replace reads
inside its own transaction and filters `tool_actions` + `session_events`
inserts against (Q2). Because the replace *already* deletes all of a
source's rows before reinserting, a filtered reinsert also prunes that
source's stale pre-cutoff rows as a side effect — reconciliation becomes an
ally rather than a threat. `response_times`, `skill_usages`, and
`hook_invocations` are inserted unfiltered and keep full history; the
`transcript_analytics_sources` registry row is untouched so the source stays
registered and reconcilable. A per-source `retained_through` column is ruled
out because `prune_transcript_analytics_sources_for_root` (storage.rs:3292)
deletes the registry row it would live on.

The watermark is **monotonic**: a run only ever advances it to
`max(existing, new_cutoff)`. Lowering the configured window must not retreat
the watermark, or rows already deleted at a stricter cutoff would become
re-insertable and the user's reclaimed space would silently come back.

*The insert filter's conformance guard is exactly the delete guard, inverted
in effect.* The predicate is: **insert unless (the row's timestamp is
conforming — `length(timestamp) = 24 AND timestamp LIKE '%Z'`) AND
(`timestamp < watermark`)**. A non-conforming timestamp (`+00:00` offsets,
truncated values, anything not 24 chars ending in `Z`) is **always
inserted**, never suppressed, because it is also never deleted — the two
guards must agree or a row could be suppressed on reinsert while its
original was retained, which is silent data loss with no delete to account
for it. `replace_transcript_analytics_snapshot` counts non-conforming
pass-throughs alongside filtered-row counts and folds both into the
`TranscriptAnalyticsReplacement` summary, so "suppressed 412, passed 3
non-conforming" is visible in reconciliation logs rather than invisible.

*Watermark advance timing.* The watermark is advanced to the run's cutoff
**after the delete-phase preflight passes and with (or immediately before)
the first chunk's commit** — not at the end of the run and emphatically not
after VACUUM. Two rules follow, and both are load-bearing:

- A run that **skips at the delete-phase preflight** must **not** advance the
  watermark. Nothing was deleted, so advancing it would suppress inserts the
  user never consented to lose — consent-free insert suppression, which is
  the one failure mode this design must not have.
- Once the first chunk commits, the watermark is at the cutoff and **stays
  there** regardless of what happens afterwards. A crash mid-run, a failed
  chunk, a skipped or failed VACUUM: none of these roll it back. Rolling it
  back would re-open the resurrection path for rows that are already
  irreversibly gone, which is the exact bug the watermark exists to prevent.

The audit record is likewise persisted **independently of the compaction
outcome** — it is written on the completion path, on the partial path, and
on the error path, and a skipped VACUUM only affects `compaction_status` and
`bytes_after`.

*One composite operation on a dedicated connection.* Q4 pins prune →
delete-preflight → VACUUM as a single operation with one progress stream.
The whole run holds one lease (Q6, acquired via
`try_begin_ingest_quiesce()` — see below) — per-chunk
lease acquire/release is rejected because it flaps
`MAINTENANCE_IN_PROGRESS` 503s and opens windows for the 120 s transcript
rescan loop (`TRANSCRIPT_RESCAN_INTERVAL_DEFAULT_SECS`, lib.rs:179) to
re-ingest mid-prune.

*Mutual exclusion is explicit, not implicit.* `begin_ingest_quiesce()`
(lib.rs:98) is a bare `RwLock::write()`: a second caller does not fail, it
**blocks unboundedly**. With `compact_database` on one button and retention
on another in the same settings section, a user who clicks Compact and then
Prune today gets a frozen second command with no feedback and a doubled
quiesce window. Retention therefore adds
`try_begin_ingest_quiesce() -> Option<IngestQuiesceGuard>` (a
`RwLock::try_write()` that only sets `MAINTENANCE_IN_PROGRESS` on success),
and **all four retention commands acquire through it**, returning the
structured skip `"another maintenance operation is running"` instead of
blocking. The frontend backs this with a single shared `maintenanceBusy`
state that disables **both** the Compact and the Prune controls while either
is in flight, so the skip is a backstop rather than the normal experience.
Like `compact_database` (lib.rs:3407), every retention command that touches
the database runs its body inside `tokio::task::spawn_blocking` — the SQL is
synchronous and would otherwise stall the async runtime for the whole lease.

*The run deletes the cutoff the user confirmed, and cannot run without one.*
A parameterless `run_retention_maintenance()` recomputes its own
`Utc::now() - window` at invocation, which means the set it deletes is not
the set the preview showed — the cutoff **moves forward** between preview and
confirm, so the run deletes strictly *more* than was previewed, including
rows that aged past the boundary while the confirm dialog was open. Small in
seconds, unbounded if the dialog sits open overnight, and in every case a
consent violation: the user approved a specific boundary date.

The signature is therefore
`run_retention_maintenance(confirmed_cutoff: String, confirmed_window_days:
i64)`, and the run uses `confirmed_cutoff` **verbatim** for the scan, the
deletes, the watermark advance, and the audit record. The backend validates
before doing anything destructive and returns the structured skip
`stale_preview` when either the confirmation is older than a small tolerance
(the confirmed cutoff trails the freshly derived one by more than a bounded
grace — value set with the other budgets) or `confirmed_window_days` no
longer matches `retention.window_days`, meaning the user changed the preset
after previewing. The UI's remedy for `stale_preview` is to re-preview, which
is cheap and honest.

This also closes a backend-side hole that UI discipline alone cannot: a
destructive run is **unreachable without a preview**, because the only source
of a valid `confirmed_cutoff` is a `RetentionPreview`. No caller — a stray
`invoke` from the console, a future automation, a bug in the confirm flow —
can prune without having produced the numbers the user was shown.

All SQL runs on a **dedicated maintenance connection**, mirroring
`vacuum_database` (storage.rs:5942) rather than `self.conn`. This matters
more than it does for VACUUM: the primary connection is a single process-wide
mutex, so a scan-and-delete on it would block every read IPC — the
always-on-top widget included — for the whole run. On a separate WAL
connection, readers keep reading; only writes are gated, and they are gated
by the quiesce lease that already exists.

*Doomed rowids are materialized once.* `tool_actions` has **no index leading
with `timestamp`** (its only timestamp-bearing index is
`idx_tool_actions_category_timestamp(category, timestamp)` from
`ensure_startup_indexes`), so a naive per-chunk `WHERE timestamp < ?` would
re-scan the table once per chunk — quadratic on 396k rows. Instead the run
does **one** pass per table into a `TEMP TABLE` of doomed rowids:

```sql
CREATE TEMP TABLE retention_doomed_tool_actions AS
SELECT rowid AS rid FROM tool_actions
 WHERE source_key IS NOT NULL
   AND length(timestamp) = 24 AND timestamp LIKE '%Z'
   AND timestamp < ?1;
```

That single scan (a) yields the **exact** preview count Q7 demands for free,
(b) makes every chunk a rowid seek, and (c) freezes the delete set so the
result reported to the user is the set that was previewed. Chunking must then
come from a predicate rather than from the statement itself:
`SQLITE_ENABLE_UPDATE_DELETE_LIMIT` is not compiled, so `DELETE … LIMIT`
does not exist.

*The chunk boundary is a value, not a `LIMIT` re-evaluation.* Two
independent `LIMIT ?1` scans over the same TEMP table are only guaranteed to
agree if their row order agrees, and an unordered `SELECT … LIMIT` carries
no such guarantee. Rather than pin `ORDER BY rid` on both scans and rely on
that, the run **materializes the boundary once per chunk** as a scalar and
deletes by comparison from both tables:

```sql
-- once, at the top of the chunk transaction
SELECT max(rid) FROM (SELECT rid FROM retention_doomed_tool_actions
                       ORDER BY rid LIMIT ?1);            -- :max

DELETE FROM tool_actions WHERE rowid <= :max
   AND rowid IN (SELECT rid FROM retention_doomed_tool_actions);
DELETE FROM retention_doomed_tool_actions WHERE rid <= :max;
```

Both statements are driven by the same `:max`, inside one transaction, so
the target chunk and the bookkeeping table can never diverge — no doomed
rowid is dropped from the TEMP table without its row being deleted, and no
row is deleted that the TEMP table still claims. The `rowid <= :max` bound
also keeps each `DELETE` a bounded rowid range rather than a full
`IN`-subquery materialization. `session_events` uses the identical shape and
gets a range seek on `idx_se_timestamp` for its scan pass.

Each chunk is its own transaction, so an interrupted run leaves completed
chunks committed and no partial row written; the next run simply recomputes
its doomed set.

*The Counting phase must visibly advance.* A single
`CREATE TEMP TABLE … AS SELECT` is one opaque statement — it emits no rows,
no intermediate result, and on a 2M-row corpus it is the longest phase the
user stares at. Reporting `Counting rows / 0%` for tens of seconds reads as
a hang. The run therefore installs a rusqlite
`Connection::progress_handler` on the maintenance connection for the
duration of the scan and uses it as a **heartbeat**, nudging the emitted
`pct` on a wall-clock cadence (and, per table, a known 0–50 / 50–100 split)
so the phase is observably alive. The handler is uninstalled before the
delete phase, which has real per-chunk progress. This applies to
`preview_retention` as well as to the run — the preview is nothing *but* the
counting phase, so it is exactly where a dead progress bar is least
acceptable.

*Timestamp non-conformance: retain and report.* S1 left an either/or; this
plan picks **retain and report**. The `length(timestamp) = 24 AND timestamp
LIKE '%Z'` guard means a row stored as `2026-04-25T00:00:00+00:00` is never
selected for deletion (its `+` sorts before `.` and would mis-compare at the
boundary). The scan pass counts such rows separately as
`skipped_nonconforming`, which is surfaced in the preview, in the run
result, and in the persisted audit record.

*WAL is bounded by a checkpoint between chunks.* Deleting a row rewrites its
entry in all seven `session_events` indexes, so WAL churn is
index-dominated. Under the quiesce lease there are no competing writers, so
the run issues `PRAGMA wal_checkpoint(TRUNCATE)` after each chunk commit,
capping WAL at roughly one chunk's dirty pages instead of the whole run's.
The **delete-phase preflight** (distinct from, and in addition to, the
VACUUM 2×-file check) therefore requires free disk ≥ one chunk's estimated
WAL bytes **plus the doomed-rowid TEMP tables' estimated bytes**, plus a
safety multiplier, and aborts before the first chunk if it fails — no rows
removed, structured skip returned, watermark **not** advanced.

The TEMP-table term is not a rounding error and it is not optional: the two
`retention_doomed_*` tables hold one 8-byte rowid per doomed row plus
b-tree overhead, so a ~700k-row prune materializes on the order of tens of
megabytes of temp storage *before* the first byte of WAL is written. Where
those bytes land depends on `PRAGMA temp_store`, which Quill sets to
`MEMORY` only on the model-analytics reader connection (storage.rs:5903) and
otherwise leaves at the build default. The maintenance connection therefore
**pins `temp_store` explicitly** — the choice is the spike's to make, but it
must be a choice: `MEMORY` trades disk for RSS and takes the TEMP term out
of the disk preflight into a memory budget; `FILE` keeps RSS flat and makes
the TEMP term a real disk requirement in the temp directory, which may not
even be the same filesystem as the database. The spike measures temp bytes
alongside WAL bytes so the preflight's constants cover both.

*Disk can run out mid-run, and the run must land somewhere honest.* A
preflight that passes at chunk 0 says nothing about chunk 400 — another
process can consume the headroom while the run is in flight. The delete
engine therefore **re-runs the free-space check every N chunks** (N from the
spike, sized so the check's `statvfs` cost is noise against chunk wall
time). On failure it does not panic and does not silently continue: it stops
cleanly at the last committed chunk, skips VACUUM, and returns
`status: "partial"` with a structured `error_reason`. The same path handles
any chunk-level SQL error. See *Mid-run failure* below.

*Mid-run failure has a name.* A chunked delete that dies at chunk 400 of
900 is neither `completed` nor `skipped` — reporting it as either lies to
the user about how much history is gone. The result vocabulary therefore
carries a third value, **`"partial"`**, with a populated `error_reason`, and
it is carried end to end: through `RetentionMaintenanceResult`, through the
`retention-maintenance-finished` payload, into the persisted
`retention.last_run` audit record, and into the UI's terminal state ("Removed
412,003 of 689,441 rows, then stopped: <reason>. Space is not yet reclaimed
— run Compact database."). The audit record is written on the **error path
too**, not only on success; a run the user cannot account for afterwards is
the failure S6 names. The watermark, already advanced at the first chunk
commit, stays where it is — the deleted rows are gone and must not resurrect.

*Connection lifecycle around VACUUM.* The maintenance connection owns the
two `retention_doomed_*` TEMP TABLEs, and `vacuum_database`
(storage.rs:5942) opens its **own** connection to run `VACUUM;`. VACUUM
rebuilds the whole file and does not tolerate the surprise of another
connection holding schema-visible temp state and an open transaction, so the
sequencing is explicit: finish the last chunk, checkpoint, **drop the
maintenance connection** (which destroys both TEMP TABLEs), and only then
invoke the VACUUM preflight and `vacuum_database`. Nothing after the delete
phase needs the maintenance connection — the watermark advance and audit
write ride the primary connection's normal `set_setting` path.

*Budgets come from a spike, not from this document.* Per Q6/OQ13, a
`retention_spike.rs` binary built in the `vacuum_spike.rs` pattern measures
delete wall time, per-chunk transaction hold, WAL bytes per chunk, TEMP-table
bytes, and **scan wall time for both tables — with and without
`idx_se_timestamp`** for a ~700k-row chunked delete on a production-shaped
fixture. Chunk size, per-chunk wall target, the WAL-bytes-per-row constant
used by the preflight, the TEMP-bytes-per-row constant, the free-space
re-check interval `N`, and the total wall-time budget are **set by that
spike's output**. The implementation must not hard-code them beforehand.

The scan measurement is not a nice-to-have. The design pays for the scan
**twice** — once in `preview_retention` and again in
`run_retention_maintenance`, which deliberately rescans under its own lease
rather than trusting the preview. `tool_actions` has no timestamp-leading
index, so its pass reads the whole table every time — **corrected by the
spike's measurement**: the planner does not fall back to a raw table scan but
walks the partial unique index `uidx_ta_owned`, which already encodes
`source_key IS NOT NULL`. It is still a full index scan and still the most
expensive single statement in the Counting phase (~693 ms of ~890 ms), so the
conclusion below is unchanged. The spike therefore sets
an explicit budget for the Counting phase, and **if scan time dominates the
run**, that is a design signal, not a tuning detail: it reopens whether the
preview should take the lease and hand the run its materialized doomed set
(one scan, one lease, a longer-held lease) instead of the current
two-scan/two-lease split. That question is answered by the spike's numbers
before the delete engine is built, not after.

*Consumer degradation is the cheap treatment (Q5).* Grounded correction to
S4's framing: `range_to_duration` (storage.rs:1230) has no `all` arm and
caps at 30 days, so at any retention window ≥ 30 days the three range-based
readers (`get_code_stats`, `get_code_stats_history`,
`get_llm_runtime_stats`) cannot ask for pruned data at all. The shipped
preset list therefore has a **30-day floor**, which makes those three
readers provably unaffected. The readers that *do* degrade are the
session-scoped ones — `get_batch_session_code_stats`, `get_session_breakdown`
(its `subagent_count` subquery), and `get_session_subagent_tree` — for
sessions that straddle or predate the cutoff. Their treatment is: a
retention banner stating the cutoff date wherever pruned-table data is
rendered, and pre-cutoff chart ranges marked or truncated rather than drawn
as zeros. No rollup aggregates (deferred follow-up bead).

*The `all` → "all retained" relabel is dropped as a shipped change.* S4
asked for it; grounded against the code it is **vacuous today**. `RangeType`
is `"1h" | "24h" | "7d" | "30d"` (src/types.ts:212) — no `all` member
exists, and `range_to_duration` (storage.rs:1230) has no `all` arm to feed.
The only "All time" affordances in the product are the two Breakdown toggles
(`BreakdownPanel.tsx:967` skills, `:1007` hooks), and they read
`skill_usages` and `hook_invocations` — tables retention never prunes.
Relabelling those would be an outright lie: their data really is all of it.
So nothing is relabelled. What ships instead is the **invariant**, recorded
in `lat.md` and in the degradation item: *any future all-time or unbounded
range reader over `tool_actions` or `session_events` must be labelled
"all retained" and carry the retention banner.* The requirement survives; the
no-op edit does not.

`subagent_count` UNIONs
`token_snapshots ∪ response_times ∪ tool_actions` and only the last is
pruned, so a pruned session's count is computed over mixed horizons and can
disagree with its drilldown — an **accepted, documented limitation**, as is
Tantivy/MCP session search surfacing a session whose SQL drilldown is empty
(MCP reads the Tantivy index, not these tables).

*Consent must be consent to a capability loss, not to a row count.*
"Delete 689,441 rows" tells a user nothing they can act on; nobody has an
intuition for what 689,441 rows of `session_events` were doing for them. The
confirm step therefore **enumerates the surfaces that degrade**, in product
language, scoped to *pre-cutoff only*:

- **Session drilldowns** — sessions older than the cutoff lose their
  event-level breakdown (`get_session_breakdown`).
- **Subagent trees** — pre-cutoff sessions no longer show which subagents
  ran or how they nested (`get_session_subagent_tree`).
- **Batch session code stats** — per-session lines-changed and file-touch
  figures for pre-cutoff sessions (`get_batch_session_code_stats`).

Plus the two honest caveats: session **search** will still find these
sessions (Tantivy, not SQL), and their subagent **counts** may not match
their now-empty drilldowns. Explicitly *not* degraded, and said so: the 30-day
range charts, all token and cost history, skills, hooks, and response times.
This ships as an `affected_surfaces` note on the preview payload — or, if the
list is genuinely static, as a fixed UI list keyed off the returned cutoff
date. Either way the **copy is owned by the settings-UI item**, which is
where it is read, and it is a close criterion there.

*Goal 2's bound is conditional, and the condition must be stated.* The goal
is that footprint stays bounded rather than growing without limit — but a
retention window is not a scheduler, and "no scheduler" is an explicit
Non-Goal. The bound therefore holds **only under periodic manual re-runs**:
one prune at a 90-day window bounds the database on that day and nothing
more; six months later, unrun, it is six months larger. The plan does not
paper over this with an implied automation. The mitigation is informational
and lives in the audit surface, which renders `last_run`'s **age against the
configured window** — "last pruned 112 days ago; window 90 days" — so drift
is legible at a glance without a timer existing anywhere in the codebase.

*Cache invalidation (OQ11, resolved).* Neither table is a `CacheTable`
today, so blast radius is currently zero — but `TableVersions` probes
max-only high-water markers that a pure DELETE never moves, so the 45 s TTL
would be the only backstop the day one of these commands gets cached. The
run therefore ends by unconditionally clearing all five in-process analytics
caches through a new `Storage::clear_analytics_caches()` and emitting
`TRANSCRIPT_ANALYTICS_UPDATED_EVENT` (lib.rs:79) so frontend hooks
revalidate. Cheap, unconditional, and correct in advance of any future
caching decision.

**Alternatives considered and rejected** (all from the spec's ranked-levers
analysis):

- *Retroactive payload eviction* (NULL `full_input`/`full_output` on
  existing rows). Rejected: `full_input IS NOT NULL` is a load-bearing WHERE
  predicate in all three code-stats readers (storage.rs:15872, :15978,
  :16081) and `get_code_stats` reads the column as a non-optional `String`,
  so NULLing payloads silently drops those rows from every code-stats
  surface unless the queries change too. It is not "zero history loss."
- *`source_key` normalization* (244-char TEXT → integer FK for only 5,569
  distinct sources). Rejected as an order of magnitude beyond the pruning it
  replaces: a ~2.0M-row, two-table, 17-index rebuild inside a startup
  migration with no quiesce, progress, or resumability. Migration 30 is the
  precedent — it had to strand `tool_actions_legacy_v30` rather than finish.
- *Per-source `retained_through` column.* Impossible as designed:
  `prune_transcript_analytics_sources_for_root` (storage.rs:3292) deletes
  the `transcript_analytics_sources` row the column would live on whenever a
  source vanishes from a completed root scan, taking the retention state
  with it.
- *One-shot DELETE without an insert-time watermark.* Rejected: the normal
  reconciliation path restores everything, so the user believes they
  reclaimed space that quietly returns.
- *Schema migration (`v35`) for the watermark.* Rejected per Q2: a v35
  database is hard-refused by older builds via `SCHEMA_TOO_NEW`, and there
  is no structural need — a `settings` row is enough and is ignored by any
  build that does not know it.
- *Rollup aggregates for pruned ranges.* Deferred (Q5, Non-Goal): a new
  table, a new migration, and five read-path changes. Filed as a follow-up
  bead, not re-litigated here.
- *Export/archive before delete.* Non-Goal (Q3): the pre-run consent preview
  with exact counts is the safeguard.
- *`dbstat` per-table footprint reporting.* `dbstat` **is** compiled
  (`SQLITE_ENABLE_DBSTAT_VTAB` is unconditional in `libsqlite3-sys 0.28.0`),
  but its full-file page walk is not paid for in MVP (Q7). Reporting is
  whole-file before/after bytes, which already includes the index pages the
  deleted rows occupied — the majority of the win for `session_events`.

---

## Affected Components

Backend (`src-tauri/src/`):

- **`storage.rs`**
  - `ensure_startup_indexes` (:1493) — add `DROP INDEX IF EXISTS
    idx_session_events_provider_source;` (Phase 1). No migration, no
    `MAX_SUPPORTED_SCHEMA_VERSION` change.
  - `replace_transcript_analytics_snapshot` (:3356) — read the retention
    watermark alongside the existing generation-key read (:3380), inside the
    same transaction, and filter the `session_events` (:3492) and
    `tool_actions` (:3551) insert loops against it. `response_times`,
    `skill_usages`, and `hook_invocations` loops are untouched. Count
    filtered rows and fold them into `TranscriptAnalyticsReplacement` /
    the reconciliation summary so a suppressed reinsert is observable in
    logs rather than invisible. Non-conforming timestamps are **always
    inserted** and counted separately in the same summary, matching the
    delete guard exactly.
  - **New retention module** (new `impl` block, or `storage/retention.rs`):
    watermark read/write helpers, the doomed-rowid scan (with the
    progress-handler heartbeat and a pinned `temp_store`), the chunked delete
    engine, the delete-phase preflight including the TEMP-table term and the
    every-N-chunks free-space re-check, the audit-record read/write, and the
    composite runner that sequences scan → preflight → delete →
    **drop the maintenance connection** → VACUUM. Everything through the
    delete phase runs on the dedicated connection; the watermark advance and
    audit write run after it is closed.
  - **New `clear_analytics_caches()`** — drains `model_analytics_cache`,
    `model_usage_overview_cache`, `model_history_cache`,
    `bucket_stats_cache`, and `context_savings_analytics_cache` (:2975–2979).
  - Reuses unchanged: `preflight_database_compaction` (:5912),
    `vacuum_database` (:5942), `available_disk_space` (:3006), `get_setting`
    (:11037), `set_setting` (:11046).
- **`lib.rs`**
  - New commands `get_retention_policy`, `set_retention_policy`,
    `preview_retention`, `run_retention_maintenance`, registered in the
    `tauri::generate_handler!` list (:4816, next to `compact_database`).
    Every command that touches the database runs inside `spawn_blocking`,
    mirroring `compact_database` (:3407).
  - **New `try_begin_ingest_quiesce() -> Option<IngestQuiesceGuard>`** beside
    `begin_ingest_quiesce()` (:98) — a `RwLock::try_write()` that sets
    `MAINTENANCE_IN_PROGRESS` only on success. All four retention commands
    acquire through it and return a structured "another maintenance operation
    is running" skip instead of blocking.
  - New progress emitter modelled on `emit_database_compaction_progress`
    (:3391), emitting `retention-maintenance-progress` /
    `retention-maintenance-finished`.
  - `compact_database` (:3407) is **not modified** — retention is a separate
    entry point that reuses the same storage primitives. It keeps
    `begin_ingest_quiesce()`; the frontend's shared `maintenanceBusy` state is
    what prevents a user from stacking it behind a retention run.
  - Retention keys join the backend-only dotted-key convention near
    `TRANSCRIPT_RESCAN_*` (:176–181).
- **`transcript_analytics.rs`** — no logic change. The watermark is enforced
  entirely inside `replace_transcript_analytics_snapshot`, so both the
  startup reconciliation driver (`run_startup_transcript_analytics_
  reconciliation`, :1141) and the forced-reparse path
  (`transcript_analytics_reingest_pending`, :717, re-armed by migrations at
  storage.rs:5717 and :5809) inherit it for free. Its per-source counters may
  optionally surface the filtered-row count.
- **`server.rs`** — **no change**. Both of its analytics write paths
  (`persist_remote_session_analytics` :855 → `store_live_session_analytics`,
  and `post_session_messages` :1628) produce `source_key IS NULL` live rows,
  which Q3 excludes from retention entirely. The existing
  `MAINTENANCE_IN_PROGRESS` 503 boundary already covers the retention lease
  because retention reuses `begin_ingest_quiesce()`.
- **`src-tauri/src/bin/retention_spike.rs`** — new, one-off, in the
  `vacuum_spike.rs` shape.

Frontend (`src/`):

- **`src/components/settings/PerformanceTab.tsx`** — a second control under
  the existing "Database maintenance" section header (:173), directly beside
  Compact: a window preset selector plus a "Review and prune" action that
  opens the consent preview and, on confirm, runs the composite operation.
  Reuses `SettingRow`, `formatBytes` (:26), and the same progress/result
  rendering idiom as `compactionDescription` (:82). Systems Pages density per
  `DESIGN.md:144` — 12–16 px internal padding, tabular figures for counts, no
  chat-style prompt. Destructive confirm styling must respect the reserved
  green/amber/red severity meter: the confirm affordance is a distinct
  destructive treatment, not a repurposed severity colour. Owns one shared
  `maintenanceBusy` state that disables **both** Compact and Prune while
  either is running, and owns the confirm step's capability-loss copy (the
  degraded-surfaces list plus the "deletion alone frees no filesystem bytes"
  sentence).
- **`src/hooks/useRetentionPolicy.ts`** — new. Loads the policy + last audit
  record, saves the window, subscribes to the two new events. Modelled on
  `useRuntimeSettings.ts` but **separate from `RuntimeSettings`**: that
  struct is saved wholesale (`PerformanceTab.update()` does `{...settings,
  ...patch}`), so folding retention into it would let a stale window
  round-trip an old value and silently reset the user's retention setting.
- **`src/types.ts`** — new `RetentionPolicy`, `RetentionPreview`,
  `RetentionMaintenanceProgress`, `RetentionMaintenanceResult`,
  `RetentionAuditRecord`. `has_subagents` (:130) / `subagent_count` (:136)
  keep their shapes; the mixed-horizon caveat is documented at the type and
  rendered as a footnote where the count is shown.
- **Consumer degradation surfaces** — session drilldowns and code-stats
  charts fed by `useSessionCodeStats`, `useSessionSubagents`, and the
  Sessions tab's breakdown. Shared "retained since <date>" banner component
  and the marked/truncated pre-cutoff chart treatment. **No `all` relabel
  ships** — `RangeType` has no `all` member (src/types.ts:212) and the two
  "All time" toggles read never-pruned tables; the requirement is recorded as
  a forward-looking invariant instead.
- **`src/mocks/ipcFixtures.ts`** — fixtures for the four new commands and
  both new events, following the `compact_database` stub (:2294) so the dev
  harness can exercise preview, no-op skip, and completed states.

Documentation:

- `lat.md/backend.md` — extend "Database compaction" (:94) with a sibling
  "Retention pruning" section; correct the stale "MCP-powered session
  search" claim in "Session Indexing" (:237) noted by the Spec Review.
- `lat.md/data-flow.md` — extend "Database Maintenance Pipeline" (:28) with
  the retention path.
- `lat.md/frontend.md` — the new hook and the Performance settings control.

---

## Data Model

**No schema migration. No `MAX_SUPPORTED_SCHEMA_VERSION` bump. No `v35`.**
All persistent retention state lives in three rows of the existing
`settings` table (`key TEXT PRIMARY KEY, value TEXT NOT NULL`,
storage.rs:3746), using the established backend-only dotted-key convention
(`transcript_rescan.enabled`, lib.rs:176).

| Key | Value | Meaning |
| --- | --- | --- |
| `retention.window_days` | decimal integer as TEXT, e.g. `"90"`; row absent or literal `"never"` | Configured window. **Absent is the default on every existing and new database** and means never prune. |
| `retention.watermark` | RFC3339 UTC, exactly 24 chars, e.g. `"2026-04-25T00:00:00.000Z"`; row absent | Insert-time cutoff. Absent means no filtering. Advanced monotonically. |
| `retention.last_run` | JSON object (below) | Durable audit record of the most recent run. |

`retention.last_run` value schema (a single JSON object; `schema` is a
forward-compatibility discriminator so a later shape can be added without a
migration):

```json
{
  "schema": 1,
  "status": "completed",
  "reason": null,
  "error_reason": null,
  "window_days": 90,
  "cutoff": "2026-04-25T00:00:00.000Z",
  "ran_at": "2026-07-24T13:02:11.412Z",
  "deleted": { "tool_actions": 165912, "session_events": 523847 },
  "skipped_nonconforming": { "tool_actions": 0, "session_events": 0 },
  "bytes_before": 7544053760,
  "bytes_after": 5610209280
}
```

`status` is `"completed"`, `"partial"`, or `"skipped"`. The first and last
match `DatabaseCompactionResult`'s vocabulary (storage.rs:2988); `"partial"`
is retention-specific and means *some chunks committed, then the run stopped*
— disk exhausted mid-run, or a chunk-level SQL error. `"partial"` always
carries a populated `error_reason` (`reason` stays the skip-reason slot and
is `null` for a partial). The alternative shape — `completed` plus an
`interrupted` flag plus `error_reason` — is rejected: a status enum that has
to be read together with a boolean to know what happened is a status enum
that will be read wrong.

A skipped run is still recorded — "I tried on this date and nothing happened,
because X" is exactly the question S6 says must have an answer after the
toast is gone. So is a partial run, and the record is written on the **error
path** as well as the success path. A malformed or unparseable
`retention.last_run` is treated as absent and logged at `warn`; it never
blocks a run.

**Cutoff derivation.** `cutoff = (Utc::now() - Duration::days(window_days))`
formatted as `%Y-%m-%dT%H:%M:%S%.3fZ` so it is byte-comparable against the
stored 24-char timestamps. It is derived **once, by `preview_retention`**,
returned to the caller, and handed back to `run_retention_maintenance` as
`confirmed_cutoff`. The run does not re-derive it: it validates the
confirmation (freshness tolerance, window unchanged) and then uses the
confirmed value verbatim for the scan, the deletes, the watermark advance,
and the audit record. The consequence is that the run is internally
consistent *and* identical to what the user approved — a re-derived cutoff
would sit later than the previewed one and delete rows the preview never
counted.

**Watermark monotonicity and advance timing.** `set` is
`watermark = max(existing, new_cutoff)`. Changing `retention.window_days`
alone never moves the watermark; only a run that actually deleted rows does.
Precisely: the advance happens **after the delete-phase preflight passes and
with the first chunk's commit**, so a run that skips at preflight leaves the
watermark untouched (nothing was deleted, so nothing may be suppressed), and
a run that commits any chunk leaves the watermark advanced permanently —
`"partial"` runs included, and regardless of whether VACUUM ran, was skipped,
or failed. Setting the window to `never` clears
`retention.window_days` but **leaves the watermark in place**, because rows
already deleted must stay deleted; the settings surface says so explicitly.

**Downgrade safety.** An older build reads the `settings` table normally and
simply does not know these three keys, so it opens the database without
complaint. Its `replace_transcript_analytics_snapshot` has no filter, so a
source it reparses will reinsert that source's pre-cutoff rows — the
accepted and documented downgrade behaviour. Nothing is corrupted; the user
loses only the durability guarantee while running the older build, and a
subsequent run on the newer build re-prunes.

**Out-of-process writers.** The watermark is read from the database inside
the writer's own transaction, never from an in-memory flag, so it is
correct across processes by construction. In practice no out-of-process
writer needs it: the HTTP server and widget paths write only `source_key IS
NULL` live rows (`store_live_session_analytics`, storage.rs:15200;
`ingest_session_events`, :16375), which retention excludes outright.

**Preset list.** `never` (default), `30d`, `90d` (recommended, shown as the
default *choice* but not the default *state*), `180d`, `365d`. The 30-day
floor is not arbitrary: `range_to_duration` (storage.rs:1230) caps every
range-based reader at 30 days, so a floor of 30 keeps `get_code_stats`,
`get_code_stats_history`, and `get_llm_runtime_stats` provably unaffected.

**Temp objects.** `retention_doomed_tool_actions` and
`retention_doomed_session_events` are `TEMP TABLE`s on the dedicated
maintenance connection, which pins `PRAGMA temp_store` explicitly rather
than inheriting the build default. They vanish when that connection is
dropped — which happens deliberately **before** `vacuum_database` is invoked
— and never touch the main schema. Their estimated size is a term in the
delete-phase preflight (disk or memory, depending on the pinned
`temp_store`), not an unbudgeted allocation.

---

## API / Interface Changes

Four new Tauri commands, registered beside `compact_database` in the
`generate_handler!` list (lib.rs:4816). **No existing command signature or
response shape changes**, including `compact_database` itself and
`set_runtime_settings`.

- **`get_retention_policy() -> RetentionPolicy`** — cheap settings reads
  only, no scan, no quiesce.

  ```rust
  pub(crate) struct RetentionPolicy {
      window_days: Option<i64>,      // None = never
      watermark: Option<String>,     // RFC3339, None = no filtering
      last_run: Option<RetentionAuditRecord>,
  }
  ```

- **`set_retention_policy(window_days: Option<i64>) -> RetentionPolicy`** —
  validates against the preset list and **rejects anything else with an
  error**: only `30`, `90`, `180`, `365`, and `None` are accepted. This
  validation *is* the 30-day floor — the floor is the guarantee that
  `get_code_stats`, `get_code_stats_history`, and `get_llm_runtime_stats` can
  never ask for pruned data (`range_to_duration` caps at 30 days,
  storage.rs:1230), so a `7` slipping through the command boundary would
  silently invalidate the whole "those three readers are provably unaffected"
  argument. Writes `retention.window_days` and returns the refreshed policy.
  Never touches the watermark and never deletes anything.

- **`preview_retention() -> RetentionPreview`** — acquires the quiesce lease
  via `try_begin_ingest_quiesce()` for the duration of the scan only, emits
  progress (heartbeat-driven, see the Counting phase note), runs the
  doomed-rowid scan on the dedicated connection, and returns **exact** counts
  plus the cutoff the run must be handed back. Uses the policy's configured
  window; returns a skip when the window is `never`, and the structured
  "another maintenance operation is running" skip when the lease is held.

  ```rust
  pub(crate) struct RetentionPreview {
      status: &'static str,          // "ready" | "skipped"
      reason: Option<String>,
      cutoff: Option<String>,
      window_days: Option<i64>,
      tool_actions_rows: i64,
      session_events_rows: i64,
      tool_actions_nonconforming: i64,
      session_events_nonconforming: i64,
      everything_older: bool,        // cutoff covers every owned row
      bytes_before: u64,
      affected_surfaces: Vec<String>, // capability loss, pre-cutoff only
  }
  ```

  `everything_older` is what drives S1's explicit-loss confirmation copy. A
  preview with both row counts at `0` returns `status: "skipped"` with a
  structured reason (fresh install, or nothing older than the cutoff) — the
  no-op path Q4 requires.

  `cutoff` is not decoration: it is the token the confirm step hands to
  `run_retention_maintenance`, and it is the only way to obtain one.
  `affected_surfaces` carries the capability-loss list the confirm step must
  show (session drilldowns, subagent trees, batch session code stats — all
  pre-cutoff only). If the list proves genuinely static it may instead be a
  fixed UI list keyed off `cutoff`; either way the copy is a close criterion
  of the settings-UI item, not an afterthought.

- **`run_retention_maintenance(confirmed_cutoff: String,
  confirmed_window_days: i64) -> RetentionMaintenanceResult`** — the single
  composite operation (Q4): one `try_begin_ingest_quiesce()` lease held
  across scan → delete-phase preflight → **watermark advance at the first
  chunk commit** → chunked deletes (with the every-N-chunks free-space
  re-check) → close the maintenance connection → VACUUM preflight → VACUUM →
  audit write → cache clear.

  The two parameters are the binding to the user's consent. The run
  **validates them first**: if `confirmed_window_days` no longer equals
  `retention.window_days`, or `confirmed_cutoff` trails a freshly derived
  cutoff by more than the tolerance, it returns the structured skip
  `stale_preview` and does nothing. Otherwise it uses `confirmed_cutoff`
  verbatim throughout. It still **rescans** under its own lease — the lease
  was released between the two calls, so the row *set* may have changed —
  but it rescans at the confirmed boundary, so drift can only be additive at
  the recent end and can never pull rows older than what the user approved.
  Because a valid `confirmed_cutoff` comes only from a `RetentionPreview`,
  the backend itself guarantees no destructive run without a preview.

  ```rust
  pub(crate) struct RetentionMaintenanceResult {
      status: &'static str,   // "completed" | "partial" | "skipped"
      reason: Option<String>,        // skip reason; None otherwise
      error_reason: Option<String>,  // populated iff status == "partial"
      cutoff: Option<String>,
      window_days: Option<i64>,
      tool_actions_deleted: i64,
      session_events_deleted: i64,
      tool_actions_nonconforming: i64,
      session_events_nonconforming: i64,
      compaction_status: &'static str, // "completed" | "skipped"
      compaction_reason: Option<String>,
      bytes_before: u64,
      bytes_after: u64,
  }
  ```

  `status: "partial"` means chunks committed and then the run stopped —
  mid-run disk exhaustion or a chunk-level SQL error. It reports the rows
  actually deleted, carries `error_reason`, forces
  `compaction_status: "skipped"` (VACUUM is not attempted after a partial),
  and is persisted to `retention.last_run` on the error path exactly like a
  success. The watermark, advanced at the first chunk commit, stays advanced.

  The `status` / `reason` / `bytes_before` / `bytes_after` quartet
  deliberately mirrors `DatabaseCompactionResult` (storage.rs:2988) so the
  UI reuses its rendering. `compaction_status` is reported **separately**
  from `status`: S2 requires that a failed VACUUM preflight still reports the
  rows that were deleted, so `status: "completed"` with
  `compaction_status: "skipped"` and `bytes_after == bytes_before` is a
  legitimate, expected outcome meaning "rows removed, bytes not yet
  reclaimed."

  Skip reasons, all structured, all leaving the watermark untouched because
  none of them deletes a row: retention disabled (`never`); nothing older
  than the cutoff; `stale_preview` (confirmation too old, or the window
  changed after previewing); `"another maintenance operation is running"`
  (`try_begin_ingest_quiesce()` returned `None`); delete-phase disk/WAL/TEMP
  preflight failed; dedicated connection could not be opened.

**Events**, mirroring the compaction pair (lib.rs:3386–3399, :3426):

- `retention-maintenance-progress` — payload
  `{ phase: &'static str, pct: u8 }`, identical in shape to
  `DatabaseCompactionProgress`. Phases:
  `Counting rows` → `Checking disk space` → `Removing old rows` (pct
  advances per chunk, so a 700k-row delete visibly moves) →
  `Compacting database` → done. `Counting rows` has no natural progress
  signal — it is one `CREATE TEMP TABLE … AS SELECT` — so its `pct` is
  driven by a `Connection::progress_handler` heartbeat on a wall-clock
  cadence rather than left pinned at zero.
- `retention-maintenance-finished` — payload is the
  `RetentionMaintenanceResult`, including the `"partial"` case.
- `preview_retention` reuses `retention-maintenance-progress` for its
  counting phase so the UI needs only one listener pair.

The two event-name constants, the emitter helper, and the
`generate_handler!` registration are **shipped as their own small item**
ahead of both the preview command and the composite command, because both
emit through them and neither should own them.

**TypeScript** (`src/types.ts`): `RetentionPolicy`, `RetentionPreview`,
`RetentionMaintenanceProgress`, `RetentionMaintenanceResult`,
`RetentionAuditRecord`, exported and consumed by
`src/hooks/useRetentionPolicy.ts`. The existing inline
`CompactDatabaseProgress` / `CompactDatabaseResult` types in
`PerformanceTab.tsx:14–24` stay where they are; retention does not refactor
them.

**Settings UI contract**: the Compact control keeps its exact current
behaviour and copy. Retention adds one preset selector plus one action, both
in the same "Database maintenance" section, and the preview is a confirm
step in the settings surface — not a modal chat-style prompt.

---

## Testing Strategy

Rust tests follow the existing layout: inline `#[cfg(test)] mod tests` in
`storage.rs` and `lib.rs`, plus integration tests under `src-tauri/tests/`.
Per repo rule, tests are **specified here for approval**, not pre-written.
The frontend still has no test runner (`tsc --noEmit` + `eslint` only), so
frontend items close on typecheck + lint + manual verification against the
dev IPC mock harness (`src/mocks/ipcFixtures.ts`,
`src/mocks/installBrowserMock.ts`).

**Frozen synthetic fixture** (S1's acceptance basis). A builder in the
`vacuum_spike.rs` shape — `tempfile::tempdir()`, a real schema created
through `Storage::init` so every index exists, then deterministic row
generation with **known per-month counts** across both target tables and all
three sibling owned tables, a fixed set of `source_key IS NULL` live rows,
and a fixed set of non-conforming (`+00:00`) timestamps. Assertions are
**exact**: for a fixture with a known count of pre-cutoff rows, the run
deletes exactly that many and no others.

It is shared by unit tests **and** the spike, which requires it to be a real
`pub` module rather than a `#[cfg(test)]` helper — a `src-tauri/src/bin/`
binary cannot see test-only code, and duplicating the generator would put
the acceptance numbers and the budget numbers on two drifting corpora, which
defeats the point of freezing it. The module is therefore ordinary
non-test code, `pub` within the crate, consumed by both.

Its contract follows the existing storage-test harness exactly: it sets
`QUILL_DEMO_MODE` and `QUILL_DATA_DIR` to a `TempDir` before `Storage::init`
(the `init_storage_in` pattern, storage.rs:16878), and **every test using it
is `#[serial]`** (`serial_test`, storage.rs:16869) because that env block is
process-global and concurrent tests would otherwise race each other into the
wrong database. Both requirements are explicit acceptance criteria of the
fixture item, not conventions a later reader has to infer.

Backend. Each test names the **item that carries it**, so no test is orphaned
into whichever bead happens to land last:

- **Watermark filters snapshot inserts (S5, Q2).** *(Insert-time watermark
  filtering.)* Set a watermark, drive
  `replace_transcript_analytics_snapshot` with a snapshot containing rows on
  both sides of it → only post-cutoff `tool_actions` and `session_events`
  rows land, while every `response_times`, `skill_usages`, and
  `hook_invocations` row lands unfiltered. Assert the
  `transcript_analytics_sources` registry row is present and unchanged.
- **Insert-filter conformance guard (S1, Q2).** *(Insert-time watermark
  filtering.)* With a watermark set, drive a snapshot whose `tool_actions`
  and `session_events` rows include pre-cutoff **non-conforming** timestamps
  (`+00:00`, truncated, no trailing `Z`) → every non-conforming row lands
  regardless of age, only conforming pre-cutoff rows are suppressed, and the
  replacement summary reports the suppressed count and the non-conforming
  pass-through count separately. This is the insert-side half of the
  guard-symmetry invariant; the delete-side half is the non-conformance test
  below, and the two must agree or a row can be suppressed on reinsert while
  its original was never deleted.
- **Normal-path resurrection regression (S5a).** *(Insert-time watermark
  filtering.)* Prune, then simulate the
  `--resume` append: change the source's mtime/size/hash and re-drive the
  normal reconciliation sweep → the pruned pre-cutoff rows do **not** return,
  and the post-cutoff rows are present exactly once.
- **Forced-reparse regression (S5b).** *(Insert-time watermark filtering.)*
  Prune, set
  `transcript_analytics_reingest_pending` (the marker at
  `transcript_analytics.rs:714`, re-armed by migrations at storage.rs:5717
  and :5809), run `run_startup_transcript_analytics_reconciliation` → same
  assertion. These are S5's two required tests.
- **Live rows survive the insert path (Q3, insert side).** *(Insert-time
  watermark filtering.)* With a watermark set, `store_live_session_analytics`
  and `ingest_session_events` insert `source_key IS NULL` rows on both sides
  of the cutoff → every one lands; the watermark filter is scoped to
  `replace_transcript_analytics_snapshot` and does not reach these paths.
- **Live rows survive the delete path (Q3, delete side).** *(Chunked delete
  engine.)* A fixture with `source_key IS NULL` rows on both sides of the
  cutoff → the doomed-rowid scan selects none of them and the run deletes
  zero live rows. The two halves are separate tests on separate items
  because they fail for entirely different reasons — a filter that leaked
  into the live insert path, versus a scan that dropped its `source_key IS
  NOT NULL` guard.
- **Chunked delete correctness and idempotency (S1).** *(Chunked delete
  engine.)* Run with a chunk size smaller than the doomed set → exact
  expected deletions, no over-delete of boundary rows (`timestamp == cutoff`
  is retained: the predicate is strict `<`), and the `retention_doomed_*`
  TEMP table drains in lockstep with the target table under the shared
  `:max` bound. Immediately re-run → 0 rows deleted, `status: "skipped"` with
  a reason.
- **Interrupted run leaves a consistent database (S3).** *(Chunked delete
  engine.)* Abort between chunks → committed chunks stay deleted, no partial
  row exists, and the next run completes without special handling.
- **Watermark is advanced by the first chunk, not by the finish (Q2).**
  *(Chunked delete engine.)* Kill the run after chunk *N* → `retention.
  watermark` already equals the run's cutoff; then set
  `transcript_analytics_reingest_pending` and drive a forced reparse → the
  rows deleted in those *N* chunks do **not** come back. This is the test
  that proves an interrupted destructive run is still durable; without it the
  watermark could be advanced at the end of the run and nothing would fail.
- **A preflight skip does not advance the watermark (Q2).** *(Chunked delete
  engine.)* With a mocked disk check that fails the delete-phase budget, the
  watermark is byte-identical before and after the call. Advancing it here
  would suppress inserts the user never consented to lose.
- **Mid-run failure reports `partial` (S3).** *(Chunked delete engine, with
  the surface assertion on the composite command.)* Force the every-N-chunks
  free-space re-check to fail after some chunks have committed → the run
  stops cleanly, `status` is `"partial"` with a populated `error_reason`, the
  deleted counts reflect what actually committed, `compaction_status` is
  `"skipped"`, the audit record is written on that error path, and the
  watermark stays advanced.
- **Timestamp non-conformance is retained and reported (S1, delete side).**
  *(Chunked delete engine.)* Rows with `+00:00` offsets older than the cutoff
  survive and appear in `*_nonconforming` on both the preview and the result.
- **Quiesce interaction (S3).** *(Composite retention command.)* Reuse the
  `write_arriving_during_quiesce_lands_after_unquiesce` pattern (lib.rs:4964)
  against the retention lease: a write fired into an active retention window
  is pending throughout and lands after release — never dropped, never
  rejected as a hard error.
- **Mutual exclusion is structured, not blocking (B-2).** *(Composite
  retention command.)* Hold the ingest gate, then invoke each of the four
  retention commands → each returns promptly with the structured "another
  maintenance operation is running" skip rather than blocking on
  `RwLock::write()`. Assert no command mutates anything on that path.
- **A run without a fresh preview is refused (Q7).** *(Composite retention
  command.)* Call `run_retention_maintenance` with a `confirmed_cutoff` older
  than the tolerance → `stale_preview` skip, zero rows deleted, watermark
  unmoved. Call it with a `confirmed_window_days` that no longer matches
  `retention.window_days` → same. Call it with a fresh preview's cutoff →
  the deletes use that exact cutoff string, not a re-derived one.
- **No scheduler exists (Non-Goal).** *(Composite retention command.)* Assert
  that nothing in the retention path registers a timer, interval, or
  background task — retention runs only from an explicit command invocation.
  A cheap structural guard against the most likely scope creep in this
  feature, and the reason Goal 2's bound is documented as conditional.
- **Delete-phase preflight is distinct from the VACUUM preflight (S3/Q6).**
  *(Chunked delete engine.)* With a mocked disk check that fails the
  delete-phase budget — including the TEMP-table term — the run aborts
  **before any row is removed** and returns a structured skip; with a check
  that passes the delete budget but fails the VACUUM 2× budget, rows are
  deleted, `status` is `completed`, `compaction_status` is `skipped`, and
  `bytes_after == bytes_before` (S2's third criterion).
- **Edge states (Q4).** *(Preview command.)* Fresh install (no owned rows) →
  preview 0, run skipped. Nothing older than the cutoff → preview 0, run
  skipped. Everything older than the cutoff → preview reports
  `everything_older: true` with the full counts, and the run is allowed to
  proceed.
- **Preview accuracy (Q7).** *(Preview command.)* The preview's exact counts
  equal the run's deleted counts on a quiesced fixture with no interleaved
  writes, with the run driven by the preview's own `cutoff`.
- **Watermark monotonicity (Q2).** *(Retention settings primitive.)* Advance
  the watermark to a 90-day cutoff, then call the advance helper with a
  365-day (earlier) cutoff → it does not retreat; `max(existing, new)` holds.
  Clearing `retention.window_days` to `never` leaves the watermark in place.
  This tests the primitive directly rather than through a full run, so a
  monotonicity bug surfaces at its own layer instead of as a confusing
  end-to-end failure.
- **Audit record round-trip (Q7).** *(Retention settings primitive.)* Write
  an audit record through the helper and read it back via
  `get_retention_policy` → cutoff, timestamp, status, and per-table counts
  match, and the record survives reopening the database. A corrupted or
  unparseable `retention.last_run` value parses as `None`, logs at `warn`,
  and does not block a subsequent write. Both are serialization tests of the
  primitive, not of the UI that renders it.
- **Cache invalidation (S4).** *(Analytics cache invalidation on prune.)*
  Warm all five analytics caches, invoke `clear_analytics_caches()` on the
  run's completion path, assert every cache is empty afterwards and that
  `transcript-analytics-updated` was emitted. Carried by the cache item, so
  it can land and prove itself in parallel with the delete engine.
- **Preset validation rejects everything else (30-day floor).** *(Policy
  read/write commands.)* `set_retention_policy` accepts exactly `30`, `90`,
  `180`, `365`, and `None`; every other value — `7`, `1`, `0`, `-90`, `45`,
  `i64::MAX` — is rejected with an error and leaves `retention.window_days`
  unchanged. This is the enforcement point for the 30-day floor, and the
  floor is what makes "the three range readers are provably unaffected" true
  rather than aspirational.
- **Index-drop plan proof (Phase 1).** *(Drop the redundant provider/source
  index — a permanent test owned by that item, not spike-only output.)* An
  assertion test that runs `EXPLAIN QUERY PLAN` on all three
  `(provider, source_key)` `session_events` delete statements
  (storage.rs:2225, :3339, :3457) against a schema with
  `idx_session_events_provider_source` already dropped, asserting each plan
  reports a search using `uidx_se_owned`. This pins the invariant so a future
  query change cannot silently regress to a scan.
- **`idx_se_timestamp_chain` survives (Constraints).** *(Drop the redundant
  provider/source index.)* After a full retention run plus VACUUM, assert the
  index exists and
  `get_llm_runtime_stats`'s `INDEXED BY` query still prepares — the invariant
  that retention drops exactly one index and no other.

Frontend (typecheck + lint + dev-mock verification):

- Preview → confirm → run flow renders progress from
  `retention-maintenance-progress` and a terminal state from
  `retention-maintenance-finished`, including the
  `completed`-with-`skipped`-compaction case **and the `"partial"` case**
  (rows removed, run stopped, `error_reason` shown, space not reclaimed).
- The confirm step shows the capability-loss list (session drilldowns,
  subagent trees, batch session code stats — pre-cutoff only) and the S2
  sentence that deletion alone frees no filesystem bytes.
- No-op skip renders as an explanatory state, not an error toast. So does
  `stale_preview`, whose remedy is a re-preview, and the "another maintenance
  operation is running" skip.
- `everything_older` renders the explicit total-loss confirmation.
- The shared `maintenanceBusy` state disables both Compact and Prune while
  either is running, and re-enables both on either terminal event.
- The retention banner appears on the session-scoped surfaces and a
  pruned-range chart shows the marked/truncated treatment rather than zeros.
  **No `all` relabel is expected** — there is no `all` range to relabel.
- Desktop, mobile, hover, empty, and loading states of the new control are
  checked against the Systems Pages density.

Spike (non-test, measurement only): `retention_spike.rs` prints delete wall
time, per-chunk transaction hold, WAL bytes per chunk, TEMP-table bytes,
post-checkpoint WAL size, and **scan wall time for both tables with and
without `idx_se_timestamp`** for a ~700k-row chunked delete on the shared
fixture. Its output is the source of truth for the numeric budgets,
including the Counting-phase budget and the free-space re-check interval.

---

## Risks

- **Delete-phase WAL and disk growth.** Deleting a `session_events` row
  rewrites entries in seven indexes, so WAL churn is index-dominated and the
  existing VACUUM 2×-file preflight does not cover it. *Mitigation:* a
  separate delete-phase preflight that aborts before the first chunk — and
  budgets the doomed-rowid TEMP tables as well as WAL, against an explicitly
  pinned `temp_store` — plus `PRAGMA wal_checkpoint(TRUNCATE)` after each
  chunk commit so WAL is bounded by one chunk rather than the whole run, plus
  an every-N-chunks re-check so headroom lost mid-run ends in a clean
  `"partial"` rather than a crash. The WAL- and TEMP-bytes-per-row constants
  the preflight uses come from the spike, not from an estimate.
- **Long quiesce versus widget staleness.** The always-on-top widget is the
  product's default surface, and the run holds one lease for scan + delete +
  VACUUM — on the measured corpus VACUUM alone was 82.5 s. *Mitigation:* all
  retention SQL runs on a dedicated connection, so the primary connection
  mutex is never held for the run and reads keep serving; only writes are
  gated, and hooks already retry the resulting `503`. The widget shows stale
  data for the window but does not freeze. This must be **verified against
  the widget**, not only against hook retry cost, before the UI item closes.
- **The `EXPLAIN QUERY PLAN` proof fails.** If any of the three
  `(provider, source_key)` `session_events` deletes falls back to a scan
  without `idx_session_events_provider_source`, the index stays and Phase 1
  loses ~473 MB of its ~473 MB non-destructive target. *Mitigation:* the
  proof is a gate with a pass/fail criterion run **before** the drop is
  written; a failure is reported as a finding and the drop bead closes as
  won't-do. Phase 2 is unaffected either way.
- **Spike numbers invalidate the assumed shape.** If per-chunk wall time is
  unacceptable at every viable chunk size, or WAL growth per chunk exceeds
  what a preflight can reasonably demand, the composite single-lease design
  needs revisiting. *Mitigation:* the spike is sequenced as a hard
  prerequisite of the delete engine, exactly so this is discovered before
  implementation commits. No budget is hard-coded before it runs.
- **One-machine corpus.** Every number in the spec comes from one developer
  machine with 3.5 months of history and 100% source-owned rows. A user with
  live rows, a longer history, or a Codex-heavy corpus may differ.
  *Mitigation:* acceptance is pinned to the frozen synthetic fixture, so
  corpus numbers are observations that inform defaults rather than gates. A
  second-corpus validation stays open and non-blocking.
- **`subagent_count` mixed horizons.** It UNIONs
  `token_snapshots ∪ response_times ∪ tool_actions` and only the last is
  pruned, so a pruned session's count can disagree with its own drilldown.
  *Accepted and documented* (Q5); the fix is rollup aggregates, deferred.
  Same for Tantivy/MCP search surfacing a session whose SQL drilldown is
  empty.
- **Preview counts drift before the run.** The lease is released between
  `preview_retention` and `run_retention_maintenance`, so ingest can add rows
  in between — and, if the run re-derived its own cutoff, **the boundary
  itself would drift forward**, pulling in rows that aged past it while the
  confirm dialog was open. That direction of drift is not benign: a
  later cutoff deletes a *superset* of what the preview showed, so the run
  can absolutely delete something the preview did not cover. *Mitigation:*
  the run does not derive a cutoff. It takes `confirmed_cutoff` /
  `confirmed_window_days` from the preview, refuses with `stale_preview` if
  the confirmation is past tolerance or the window changed, and rescans at
  the **confirmed** boundary under its own lease. With the boundary pinned,
  the only remaining drift is row-set drift at the recent end, which is
  strictly additive above the cutoff and therefore cannot delete anything
  older than what the user approved. The reported numbers are still what the
  run actually did.
- **Mid-run failure misreports history loss.** A chunked delete that stops
  at chunk 400 of 900 has removed real history, and calling that either
  "completed" or "skipped" tells the user something false about their
  database. *Mitigation:* the `"partial"` status with `error_reason`, carried
  through the result, the `-finished` event, the audit record, and the UI;
  the audit write on the error path; the every-N-chunks free-space re-check
  that turns the most likely cause into a clean stop rather than a crash; and
  the watermark, advanced at the first chunk, staying advanced so the deleted
  rows do not resurrect.
- **Two maintenance operations stack invisibly.** `begin_ingest_quiesce()`
  blocks rather than fails, so Compact-then-Prune (or two Prunes) would sit
  in a lock queue with no user-visible signal and a doubled quiesce window.
  *Mitigation:* `try_begin_ingest_quiesce()` plus a structured skip on all
  four retention commands, and one shared `maintenanceBusy` state disabling
  both controls in the UI. The skip is the backstop; the disabled control is
  the experience.
- **The Counting scan dominates the run.** `tool_actions` has no
  timestamp-leading index, so its doomed-rowid pass is a full table scan, and
  the design pays for it **twice** — once in the preview, once in the run.
  *Mitigation:* the spike measures scan wall time for both tables with and
  without `idx_se_timestamp` and sets an explicit Counting-phase budget
  before the delete engine is built. If scan cost dominates, the response is
  a design change, not a tuning pass: hand the run the preview's materialized
  doomed set and let the preview take the lease. That trade is named here so
  it is a decision with a trigger rather than a surprise.
- **Goal 2's bound depends on the user re-running.** Retention has no
  scheduler by Non-Goal, so a 90-day window bounds the database only on the
  days it is actually run; unrun for six months, the database is six months
  larger and the setting still reads "90 days". *Mitigation:* the audit
  surface renders `last_run`'s age against the configured window ("last
  pruned 112 days ago; window 90 days"), making drift legible without adding
  a timer. Automation stays a deliberate Non-Goal, not an oversight.
- **Rollback story.** The watermark, the window, and the audit record are
  three `settings` rows — no migration, no `MAX_SUPPORTED_SCHEMA_VERSION`
  bump, no `SCHEMA_TOO_NEW` lockout. A user can downgrade and the older build
  ignores all three. The deletion itself is irreversible, which is why the
  feature is opt-in, defaults to `never`, excludes live rows, and gates every
  destructive run behind an exact-count consent preview that the **backend
  itself enforces** — the only source of a valid `confirmed_cutoff` is a
  `RetentionPreview`, so no caller can prune without one. Owned rows remain
  transcript-backed on disk, so a user who wants history back can clear the
  watermark and force a reparse.
- **Partial-session drilldowns look broken before the degradation UI lands.**
  If the delete engine ships ahead of the consumer treatment, a pruned
  session renders as a legitimately empty drilldown. *Mitigation:* the
  degradation item is sequenced as a dependency of the settings UI, so no
  user can trigger a prune before the honest rendering exists.

---

## Sequencing

Ordered work items with explicit dependency edges; this becomes the bead
DAG. Phase 1 items are independent of Phase 2 items and can run in parallel
with them, but Phase 1 lands first because it costs no history.

- **Prove index-drop query plans.** Grep every `session_events` query
  constrained on `(provider, source_key)` — at minimum storage.rs:2225,
  :3339, :3457 — and run `EXPLAIN QUERY PLAN` for each against a schema with
  and without `idx_session_events_provider_source`, using the vendored
  `libsqlite3-sys 0.28.0` build. *Pass:* every plan searches via
  `uidx_se_owned`. *Fail:* report the finding; the drop does not ship.
  *Depends on: nothing.* Independent — start immediately, in parallel with
  the fixture below.

- **Drop the redundant provider/source index and measure.** Add `DROP INDEX
  IF EXISTS idx_session_events_provider_source;` to `ensure_startup_indexes`
  (storage.rs:1493), with a one-line doc comment on that function recording
  that it now also *drops* an index — the name says "ensure", so the second
  responsibility has to be written down or the next reader will not look for
  it — and why the drop lives there rather than in a migration.

  *Hard acceptance criteria (all must pass):*
  - `sqlite_master` no longer lists `idx_session_events_provider_source`
    after an open.
  - `idx_se_timestamp_chain` **is** present after the same open.
  - The `INDEXED BY idx_se_timestamp_chain` query at storage.rs:16542 still
    prepares.
  - The permanent `EXPLAIN QUERY PLAN` assertion test — owned by **this**
    item, not by the proof spike — passes for all three
    `(provider, source_key)` delete sites.

  *Observational, recorded but not pass/fail:* whole-file bytes before and
  after a `compact_database` run on a production-sized copy (the drop
  reclaims nothing until VACUUM), plus `DROP INDEX` wall time and the WAL
  delta the drop itself produces on that copy. These are numbers the item
  must report — a startup-path cost every user pays once — not thresholds it
  can fail against, since they are corpus-dependent by nature.

  *Depends on: Prove index-drop query plans (hard gate).*

- **Decide the tool_detail payload write policy.** Close the grep over every
  reader of `full_input`, `full_output`, and `category = 'tool_detail'`
  across `src-tauri/src/` (including `sessions.rs` and the Tantivy indexer)
  and `src/`, then commit to a forward-only policy: omit the payload columns
  for `tool_detail` rows, stop writing those rows, or keep them as-is.
  Prior evidence: the only SQL readers are storage.rs:15872/:15978/:16081,
  all gated on `category = 'code_change'`, and no frontend file references
  either column. Retroactive NULLing is out of scope. Decision-only — this
  item writes no production code. *Depends on: nothing.* Independent —
  parallel with everything above.

- **Apply the `tool_detail` payload write policy.** *Conditional.* If the
  decision above lands on "omit the payload columns" or "stop writing the
  row", implement it in the `tool_actions` insert loop
  (storage.rs:3551) and update the affected assertions. **This is the same
  insert loop the watermark filter modifies**, so the two changes collide if
  they are worked concurrently — hence a real edge rather than an implied
  one. If the decision is "keep them as-is", this item **closes as won't-do**
  with the decision recorded; that is an expected outcome, not a failure.
  *Depends on: Decide the tool_detail payload write policy, Insert-time
  watermark filtering.*

- **Frozen synthetic retention fixture.** The shared builder with known
  per-month counts across both target tables and all three sibling tables,
  plus live rows and non-conforming timestamps, created through
  `Storage::init` so every index exists. It is **`pub` non-test code**, not a
  `#[cfg(test)]` helper: `src-tauri/src/bin/retention_spike.rs` cannot see
  test-only code, and a duplicated generator would put the acceptance corpus
  and the budget corpus on separate drifting definitions. *Acceptance
  criteria:* the builder sets `QUILL_DEMO_MODE` and `QUILL_DATA_DIR` to a
  `TempDir` before `Storage::init`, following `init_storage_in`
  (storage.rs:16878); every consuming test is annotated `#[serial]`
  (`serial_test`, storage.rs:16869) because that env block is process-global;
  and the same builder is exercised from both a test and the spike binary so
  the `pub` path is proven, not assumed. *Depends on: nothing.* Independent;
  **the spike and every Phase 2 test item consume it**, so land it first.

- **Retention timing spike.** New `src-tauri/src/bin/retention_spike.rs` in
  the `vacuum_spike.rs` shape, built on the frozen fixture. Measures: delete
  wall time for ~700k rows, per-chunk transaction hold across several chunk
  sizes, WAL bytes per chunk, TEMP-table bytes for the doomed-rowid tables
  under each candidate `temp_store` setting, post-`wal_checkpoint(TRUNCATE)`
  WAL size, and **scan wall time for both tables, with and without
  `idx_se_timestamp`**. *Output fixes the numeric budgets* — chunk size,
  per-chunk wall target, WAL- and TEMP-bytes-per-row constants for the
  preflight, the free-space re-check interval `N`, the stale-preview
  tolerance, the Counting-phase budget, and the total wall-time budget.
  *Also reports a design signal:* whether the Counting scan dominates the
  run. If it does, the item's output includes an explicit recommendation on
  whether `preview_retention` should take the lease and hand the run its
  materialized doomed set instead of the current two-scan/two-lease split.
  *Depends on: Frozen synthetic retention fixture (hard — the spike consumes
  the shared builder, and its numbers are only comparable to the tests'
  numbers if they run on the same corpus).*

- **Retention settings primitive.** The three `settings` keys, their typed
  read/write helpers, `RetentionPolicy` / `RetentionAuditRecord` (including
  `"partial"` and `error_reason`), cutoff derivation, monotonic watermark
  advance, and tolerant JSON parsing for the audit record. **Carries the
  watermark-monotonicity test and the audit round-trip / corrupted-value
  tests** — these are serialization and invariant tests of the primitive
  itself, and pinning them here means a monotonicity or parsing bug fails at
  its own layer instead of surfacing as a baffling end-to-end failure in a UI
  bead. *Depends on: Frozen synthetic retention fixture (for its tests).*
  Blocks the insert-time filter, the delete engine, the event scaffolding,
  and the commands.

- **Retention event constants, progress emitter, and handler registration.**
  The `retention-maintenance-progress` / `retention-maintenance-finished`
  name constants, the emitter helper modelled on
  `emit_database_compaction_progress` (lib.rs:3391), the shared phase
  vocabulary, and the `generate_handler!` registration slot (lib.rs:4816).
  Small and mechanical, extracted deliberately: both the preview command and
  the composite command emit through this scaffolding, so leaving it inside
  either one makes the other wait on a bead that is mostly not about it.
  *Depends on: Retention settings primitive.* Blocks the preview command and
  the composite command — and, once it lands, those two genuinely do
  parallelize.

- **Insert-time watermark filtering.** Read the watermark inside
  `replace_transcript_analytics_snapshot`'s existing transaction
  (storage.rs:3380) and filter the `session_events` (:3492) and
  `tool_actions` (:3551) insert loops with the conformance-guarded predicate
  — *insert unless conforming AND older than the watermark*, so
  non-conforming timestamps always land; leave the three sibling loops
  unfiltered and the registry row untouched; count suppressed and
  non-conforming rows into the replacement summary. Carries S5's two
  regression tests (normal-path resurrection and forced reparse), the
  conformance-guard test, the sibling-table assertions, and the **insert-side
  half** of the live-rows-never-pruned test. *Depends on: Retention settings
  primitive, Frozen synthetic retention fixture.*

- **Chunked delete engine and delete-phase preflight.** The dedicated
  maintenance connection with a pinned `temp_store`, the one-pass
  doomed-rowid `TEMP TABLE` scan with the `source_key IS NOT NULL` and
  24-char-`Z` guards and the `progress_handler` heartbeat, the
  max-rid-bounded chunked delete (one `:max` per chunk transaction driving
  both the target table and the TEMP table) with per-chunk commit and
  `wal_checkpoint(TRUNCATE)`, the delete-phase disk/WAL/TEMP preflight using
  the spike's constants, the every-N-chunks free-space re-check with a clean
  abort into `"partial"`, the watermark advance at the first chunk commit,
  and the audit write on both the success and error paths. Carries the
  chunk-correctness, idempotency, interrupted-run, watermark-advance-timing
  (kill-after-chunk-*N*), preflight-skip-does-not-advance, `"partial"`,
  non-conformance, preflight-distinctness, and **delete-side half** of the
  live-rows tests. *Depends on: Retention timing spike (for the budgets),
  Retention settings primitive, Frozen synthetic retention fixture.*

- **Analytics cache invalidation on prune.** `Storage::clear_analytics_
  caches()` draining all five caches plus the
  `transcript-analytics-updated` emission, wired into the run's completion.
  **Carries the cache-invalidation test**, which belongs here rather than on
  the composite command: it is a test of this function's behaviour and can
  prove itself the moment the function exists. Resolves OQ11's open shape.
  *Depends on: Retention settings primitive.* Can run in parallel with the
  delete engine.

- **Composite retention command.**
  `run_retention_maintenance(confirmed_cutoff, confirmed_window_days)` in
  `lib.rs`, inside `spawn_blocking`: validate the confirmation (tolerance and
  window match, else `stale_preview`), take the lease via
  `try_begin_ingest_quiesce()` (else the "another maintenance operation is
  running" skip), then scan → delete preflight → chunked deletes → **close
  the maintenance connection** → VACUUM preflight → `vacuum_database` →
  audit write → cache clear. Adds `try_begin_ingest_quiesce()` beside
  `begin_ingest_quiesce()` (lib.rs:98) and routes all four retention commands
  through it. Wires the `RetentionMaintenanceResult` shape — `"partial"` with
  `error_reason`, and `compaction_status` reported separately from `status` —
  into the `-finished` emitter. `compact_database` is not touched. Carries
  the quiesce-interaction test, the mutual-exclusion test, the
  `stale_preview` refusal test, the no-scheduler assertion, and the composite
  skip-path tests. *Depends on: Chunked delete engine and delete-phase
  preflight, Analytics cache invalidation on prune, Retention event
  constants, progress emitter, and handler registration.*

- **Preview command.** `preview_retention` sharing the scan with the run,
  emitting the counting phase through the shared emitter (heartbeat-driven,
  so it visibly advances), returning exact counts plus `cutoff` — the token
  the run requires — `everything_older`, the non-conformance counts, and
  `affected_surfaces`, and returning the structured no-op skip for fresh
  installs and nothing-older databases. Carries the preview-accuracy and
  edge-state tests. *Depends on: Chunked delete engine and delete-phase
  preflight (the scan is shared), Retention event constants, progress
  emitter, and handler registration.* Runs in parallel with the composite
  command — genuinely, now that neither owns the event scaffolding the other
  needs.

- **Policy read/write commands and TypeScript types.**
  `get_retention_policy` / `set_retention_policy` with preset validation that
  **rejects** anything outside 30/90/180/365/`None` (the 30-day floor,
  justified by `range_to_duration`, storage.rs:1230), the five new
  `src/types.ts` types — including `"partial"`, `error_reason`, and
  `affected_surfaces` — and `src/mocks/ipcFixtures.ts` stubs for all four
  commands and both events, covering the preview, no-op, `stale_preview`,
  busy, completed, completed-with-skipped-compaction, and `"partial"` states.
  Carries the preset-rejection test. *Depends on: Retention settings
  primitive.* Can run in parallel with the delete engine.

- **Consumer degradation treatment.** The retention banner stating the
  cutoff on session-scoped surfaces and marked or truncated pre-cutoff chart
  ranges. Documents the `subagent_count` mixed-horizon limitation at the type
  and as a rendered footnote, and the Tantivy-hit-with-empty-drilldown case.
  Note the grounded scope: `range_to_duration` caps range readers at 30 days,
  so with the 30-day preset floor only `get_batch_session_code_stats`,
  `get_session_breakdown`, and `get_session_subagent_tree` degrade.
  **No `all` → "all retained" relabel ships**: `RangeType` is
  `"1h" | "24h" | "7d" | "30d"` (src/types.ts:212) with no `all` member, and
  the two "All time" toggles (`BreakdownPanel.tsx:967`, `:1007`) read
  `skill_usages` / `hook_invocations`, which retention never prunes —
  relabelling them would be false. What this item does instead is **record
  the invariant** (in `lat.md` and as a comment where the range vocabulary is
  defined): any future all-time or unbounded range reader over `tool_actions`
  or `session_events` must be labelled "all retained" and carry the banner.
  *Depends on: Policy read/write commands and TypeScript types.* Blocks the
  settings UI so no user can prune before the honest rendering exists.

- **Retention control in Performance settings.** The preset selector, the
  preview-and-confirm flow, and the progress/terminal rendering in
  `PerformanceTab.tsx` beside the existing Compact control, plus
  `src/hooks/useRetentionPolicy.ts` kept separate from the wholesale-saved
  `RuntimeSettings`, plus the shared `maintenanceBusy` state that disables
  **both** Compact and Prune while either is running. Systems Pages density
  per `DESIGN.md:144`; destructive confirm styling must not repurpose the
  reserved severity colours. *Close criteria:*
  - Typecheck and lint green.
  - The confirm step states the **capability loss** — session drilldowns,
    subagent trees, and batch session code stats, pre-cutoff only — from
    `affected_surfaces` or a static list keyed off the cutoff.
  - The confirm step carries the S2 sentence in so many words: **deletion
    alone frees no filesystem bytes; compaction is required**, and the
    completed-with-skipped-compaction terminal state repeats it.
  - Preview, no-op skip, `stale_preview`, busy-skip, `everything_older`,
    completed, completed-with-skipped-compaction, and `"partial"` all render
    in the dev IPC mock.
  - `maintenanceBusy` observably disables both controls and releases on
    either terminal event.
  - Desktop, mobile, hover, empty, and loading states checked.
  - Widget staleness during an active lease observed and recorded.

  *Depends on: Preview command, Composite retention command, Consumer
  degradation treatment.*

- **Audit record surfacing.** Render `last_run` — cutoff, run date, status
  (including `"partial"` with its `error_reason`), rows removed per table,
  bytes before/after — in the Performance settings surface so "what did I
  delete and when" has an answer after the toast is gone. Also renders
  `last_run`'s **age against the configured window** ("last pruned 112 days
  ago; window 90 days"), which is the only mitigation Goal 2's
  no-scheduler bound gets. The round-trip and corrupted-value tests live on
  the settings primitive, not here; this item is rendering. *Depends on:
  Retention control in Performance settings.* Small; may be folded into the
  UI item if that keeps the bead honest.

- **File the deferred follow-up beads.** Rollup aggregates for pruned ranges
  (Q5 / Non-Goal); export or archive of pruned rows (Q3 / Non-Goal);
  `model_usage_observations` retention (Non-Goal, 563 MB base + ~510 MB
  indexes, third-largest consumer); second-corpus validation (OQ14,
  non-blocking); `dbstat` per-table footprint reporting (Q7, deferred).
  File-only. *Depends on: nothing.* Independent.

- **Document the feature in lat.md and pass `lat check`.** Extend
  `lat.md/backend.md` with a "Retention pruning" section beside "Database
  compaction" (:94) covering the settings-key data model, the watermark
  invariant (including its advance-at-first-chunk timing), the one-index
  exception, the `"partial"` status, and the new commands and events; extend
  `lat.md/data-flow.md`'s "Database Maintenance Pipeline" (:28) with the
  retention path; add the new hook and control to `lat.md/frontend.md`;
  record the all-range invariant from the degradation item; add test-spec
  sections with `// @lat:` refs per repo convention; and correct the stale
  `tool_actions`-backs-"MCP-powered session search" claim in
  `lat.md/backend.md` § Session Indexing (:237).

  Include **one line recording the learning-pipeline expiry condition**:
  `learning.rs` today reads transcript JSONL via
  `crate::sessions::extract_messages_from_jsonl` (learning.rs:633) and never
  touches `tool_actions`, which is why retention has no learning stakeholder
  — and a future learn pipeline that sources from `tool_actions` instead
  becomes one, at which point this analysis must be redone. Cheap to write
  now; impossible to reconstruct later from the absence of a note.

  *Close criterion:* `lat check` green. *Depends on: every implementation
  item above.* Final item.

---

## Alignment fixes applied

- **(A-F1 must)** `run_retention_maintenance` now takes
  `confirmed_cutoff: String, confirmed_window_days: i64` and uses the
  previewed cutoff verbatim, with a structured `stale_preview` skip when the
  confirmation is past tolerance or the window changed. Deleted the backwards
  claim that forward drift "cannot make the run delete something the preview
  did not cover" — a later cutoff deletes a superset — and replaced it with
  the pinned-boundary argument. Also records that this makes a destructive
  run backend-side unreachable without a preview. *(Architecture, API, Data
  Model, Testing, Risks, Sequencing.)*
- **(A-F2 / B-1 must)** Watermark advance moved to *after the delete-phase
  preflight passes, at the first chunk's commit* — never after VACUUM. A
  preflight-skip run does not advance it (consent-free insert suppression);
  a committed chunk advances it permanently, including on `"partial"` and on
  a skipped or failed VACUUM. Audit record persisted independently of the
  compaction outcome. Added the kill-after-chunk-*N* → watermark-equals-cutoff
  → forced-reparse-does-not-restore test, and a preflight-skip-does-not-
  advance test. *(Architecture, Data Model, API, Testing, Sequencing.)*
- **(A-F3 must)** Stated the insert filter as *insert unless (conforming:
  `length = 24 AND LIKE '%Z'`) AND `timestamp < watermark`* — non-conforming
  rows always insert — and required them counted alongside filtered-row
  counts in the replacement summary. Added the conformance-guard test and
  named the guard-symmetry invariant against the delete-side test.
  *(Architecture, Affected Components, Testing, Sequencing.)*
- **(B-2 must)** Added `try_begin_ingest_quiesce()` (grounded on
  `begin_ingest_quiesce()`'s unbounded `RwLock::write()`, lib.rs:98) with a
  structured "another maintenance operation is running" skip on all four
  retention commands, one shared frontend `maintenanceBusy` state disabling
  both Compact and Prune, and an explicit statement that retention commands
  run under `spawn_blocking` like `compact_database` (lib.rs:3407). Added the
  mutual-exclusion test. *(Architecture, Affected Components, API, Testing,
  Sequencing.)*
- **(B-3 must)** Added `"partial"` status with `error_reason` (rejecting the
  completed+interrupted+error_reason alternative explicitly), carried through
  the result, the `-finished` event, the audit record, and the UI; added the
  every-N-chunks free-space re-check aborting cleanly into partial; required
  the audit write on the error path; and required the maintenance connection
  (and its `retention_doomed_*` TEMP TABLEs) to be closed **before**
  `vacuum_database` is invoked. *(Architecture, Data Model, API, Testing,
  Risks, Sequencing.)*
- **(B-4 must)** Resolved the spike/fixture contradiction: the fixture
  builder is now `pub` non-test code consumed by both the spike binary and
  the tests, with `QUILL_DATA_DIR` + `#[serial]` (the `init_storage_in` /
  `serial_test` pattern, storage.rs:16878/:16869) spelled out as acceptance
  criteria; the retention timing spike's "*Depends on: nothing*" is corrected
  to a hard dependency on the fixture. *(Testing, Sequencing.)*
- **(B-5 must)** Extracted "Retention event constants, progress emitter, and
  handler registration" as its own item depending only on the settings
  primitive; both the Preview command and the Composite command now depend on
  it, and the previously false "Preview can run in parallel with the
  composite command" claim is replaced by a parallelism that is now real.
  *(API, Sequencing.)*
- **(A-F4 should)** Chunk deletes are now bounded by a single `:max` rowid
  selected once per chunk transaction and applied to both the target table
  and the TEMP table, replacing the two independent unordered `LIMIT` scans.
  *(Architecture, Testing, Sequencing.)*
- **(A-F5 should)** Dropped the `all` → "all retained" relabel as a shipped
  change — `RangeType` is `"1h" | "24h" | "7d" | "30d"` (src/types.ts:212)
  and the only "All time" toggles (`BreakdownPanel.tsx:967`, `:1007`) read
  never-pruned `skill_usages` / `hook_invocations` — and recorded the
  requirement instead as a forward-looking invariant for any future all-range
  reader over the two pruned tables. *(Architecture, Affected Components,
  Testing, Sequencing.)*
- **(A-F6 should)** The consent preview now enumerates capability loss —
  session drilldowns, subagent trees, batch session code stats, pre-cutoff
  only — via an `affected_surfaces` payload note or a static list keyed off
  the cutoff, with the copy assigned to the settings-UI item as a close
  criterion. *(Architecture, API, Testing, Sequencing.)*
- **(A-F7 should)** Delete-phase preflight now budgets the doomed-rowid TEMP
  tables alongside WAL, the maintenance connection pins `PRAGMA temp_store`
  explicitly (grounded on storage.rs:5903), and the spike measures temp bytes
  alongside WAL bytes. *(Architecture, Data Model, Testing, Risks,
  Sequencing.)*
- **(A-F8 should)** The Counting phase is driven by a rusqlite
  `Connection::progress_handler` heartbeat so it visibly advances on both the
  preview and the run, instead of sitting at zero through one opaque
  `CREATE TEMP TABLE … AS SELECT`. *(Architecture, API, Sequencing.)*
- **(A-F9 should)** The index-drop item now also records `DROP INDEX` wall
  time and the drop's own WAL delta on a production-sized copy, not only the
  post-VACUUM whole-file delta. *(Architecture, Sequencing.)*
- **(A-F10 should)** Goal 2's bound is stated as conditional on periodic
  manual re-runs (no scheduler, by Non-Goal), mitigated by the audit surface
  rendering `last_run` age against the configured window ("last pruned 112
  days ago; window 90 days"). *(Architecture, Risks, Sequencing.)*
- **(A-F11 should)** The lat.md item now includes a line recording the
  learning-pipeline expiry condition: `learning.rs` reads transcript JSONL
  today (learning.rs:633) and a future learn pipeline over `tool_actions`
  becomes a retention stakeholder. *(Grounding facts, Sequencing.)*
- **(B-6 should)** Added the conditional item "Apply the `tool_detail`
  payload write policy", depending on both the decision item and the
  insert-time watermark filtering item because both edit the same insert loop
  (storage.rs:3551), with an explicit close-as-won't-do branch.
  *(Sequencing.)*
- **(B-7 should)** Retest carriers assigned: audit round-trip and
  corrupted-value tests moved to the settings primitive, which also gains the
  watermark-monotonicity test; the cache-invalidation test moved to the cache
  item; the `EXPLAIN QUERY PLAN` assertion is now a permanent test owned by
  the index-drop item rather than spike-only output; the live-rows-never-
  pruned test is split into an insert-side half (insert-filter item) and a
  delete-side half (delete-engine item). Every backend test now names its
  carrying item. *(Testing, Sequencing.)*
- **(B-8 should)** The index-drop item's acceptance is split into hard
  criteria (`sqlite_master` no longer lists
  `idx_session_events_provider_source`; `idx_se_timestamp_chain` present; the
  `INDEXED BY` query at storage.rs:16542 still prepares; the EQP assertion
  passes) and explicitly observational byte/time measurements.
  *(Sequencing.)*
- **(B-9 should)** The spike now measures scan wall time for both tables with
  and without `idx_se_timestamp` and sets a Counting-phase budget; the
  preview+run double-scan concern is named, with an explicit trigger to
  revisit whether the preview should take the lease if scan time dominates.
  *(Architecture, Testing, Risks, Sequencing.)*
- **(B-10 should)** Close criteria strengthened: the UI item must carry the
  S2 copy ("deletion alone frees no filesystem bytes; compaction required"),
  a no-scheduler assertion was added (no timer registration anywhere in the
  retention path), and a `set_retention_policy` preset-rejection test pins
  the 30-day floor to only 30/90/180/365/`None`. *(API, Testing,
  Sequencing.)*
- **(B-nit should)** The index-drop item now lands with a one-line doc
  comment on `ensure_startup_indexes` recording that it also *drops* an
  index, since the function name no longer tells the whole story.
  *(Architecture, Sequencing.)*
