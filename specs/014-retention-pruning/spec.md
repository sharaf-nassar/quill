# Spec: retention-pruning (database footprint reduction)

**Historical evidence note.** The spike binaries referenced by this completed spec were removed on 2026-08-03. Their recorded results remain authoritative; named paths and invocations are not runnable.

## Problem Statement

Quill's SQLite database grows without bound. Nothing in the app ever deletes a
transcript-analytics row: every tool call and every JSONL line Quill has ever
seen is retained forever, and the only lifecycle machinery that exists today
(`cleanup_old_observations` for the learning `observations` table, and
`aggregate_and_cleanup` for token/usage hourly rollups) does not touch the two
tables that actually dominate the file.

Feature 013 (analytics-query-perf) shipped a migration that drops the dead
`tool_actions_legacy_v30` archive plus a user-triggered `compact_database`
VACUUM path. Its own Spec Review flagged that this oversells "disk usage
drops": the dead table is ~0.87 GB against live tables that are far larger and
still growing. 013 Clarification Q6 explicitly deferred retention/pruning to a
follow-up (bead `quill-2ag`, planning stub) with no downstream dependencies.
This spec is that follow-up.

**Measured 2026-07-24**, read-only (`immutable=1`) against the real
`~/.local/share/com.quilltoolkit.app/usage.db` (7,544,053,760 bytes), using
`dbstat` for per-object page totals:

| Object | Rows | Base table | Indexes | Total |
| --- | ---: | ---: | ---: | ---: |
| `tool_actions` | 396,241 | 1,719.9 MB | ~432 MB | **~2.15 GB** |
| `session_events` | 1,598,902 | 783.9 MB | ~1,785 MB | **~2.57 GB** |

**The 013 figures understate the problem by roughly 2×.** 013 quoted
"1.69 GB + 0.77 GB = 2.46 GB", which is base-table pages only. With indexes
these two tables account for **~4.7 GB of the 7.54 GB file (~62%)**. Notable
sub-facts:

- `session_events` is index-dominated: `uidx_se_owned` (555.4 MB),
  `idx_session_events_provider_source` (472.7 MB), `idx_se_timestamp_chain`
  (227.0 MB), `idx_se_chain` (198.8 MB), `idx_se_provider_session_sidechain`
  (137.8 MB), `idx_se_provider_chain_timestamp` (133.7 MB), `idx_se_timestamp`
  (59.3 MB). Its indexes are **2.3× its data**.
- `tool_actions` is payload-dominated: `full_input` + `full_output` total
  1,236,988,048 bytes — **~72% of the base table**. `summary` adds 44 MB.
- `source_key` averages **244 characters** and is stored on every row of both
  tables, plus repeated inside `uidx_se_owned`,
  `idx_session_events_provider_source`, `uidx_ta_owned`, and
  `idx_tool_actions_provider_source` — for only **5,569 distinct sources**
  (`transcript_analytics_sources` row count).
- Growth is not slowing: `session_events` holds 875,534 rows stamped `2026-07`
  versus 42,973 for `2026-03`; `tool_actions` payload bytes per month run
  46 MB (Mar), 474 MB (Apr), 152 MB (May), 47 MB (Jun), 519 MB (Jul).
- The local DB still carries `tool_actions_legacy_v30` at 869.3 MB (migration
  34 has not yet been applied to this file), so the *post-013* steady state is
  still ~4.7 GB of live analytics.

**Who this hurts:** every long-running Quill user. The app is a local desktop
tool whose whole value is long history, so the failure mode is guaranteed
rather than hypothetical — the file only grows. Symptoms: multi-GB disk
consumption a user never opted into, worse page locality on every analytics
scan (013's root cause #1), a `compact_database` VACUUM whose wall time scales
with file size (82.5 s measured on a 7.45 GB fixture) and whose free-disk
preflight demands ~2× the file, and backup/sync tools carrying a file that
doubles yearly.

**Why now:** 013 shipped the quiesce + compaction machinery that pruning needs
to compose with, and it shipped the honest admission that the space it reclaims
is a rounding error next to these two tables.

**Framing:** this feature is *database footprint reduction* with ranked levers,
not row deletion for its own sake. It ships in two phases inside one epic
(see Clarifications Q1). **Phase 1 is non-destructive:** drop
`idx_session_events_provider_source` (~473 MB) once `EXPLAIN QUERY PLAN` proves
the snapshot deletes seek via the partial `uidx_se_owned`, and settle the
`full_input`/`full_output` write-policy question. **Phase 2 is destructive:**
opt-in, age-based retention over `tool_actions` and `session_events`, enforced
at insert time and handed off to the existing compaction path. Phase 1 costs no
history, so it is sequenced first.

## Goals

- **Non-destructive reclaim first (Phase 1).** Reclaim footprint that costs no
  history at all: dropping `idx_session_events_provider_source` returns
  ~473 MB on the measured corpus, subject to `EXPLAIN QUERY PLAN` proof that
  the snapshot-replace deletes still seek via the partial `uidx_se_owned`.
  This lands before any destructive step and is measured independently of it.
- **Bounded steady-state size for the two target tables (Phase 2).** After
  retention is applied and a compaction runs, the on-disk footprint of
  `tool_actions` + `session_events` (data *and* indexes) is a function of the
  retention window, not of total install age. A user who has run Quill for two
  years under the same usage rate should see roughly the same footprint for
  these two tables as one who has run it for one. **Scope caveat:** this is
  explicitly *not* a bound on the whole file — a perfect prune of both tables
  still leaves ~5 GB of other, still-growing tables
  (`model_usage_observations`, `observations`, the snapshot tables). The whole
  file is out of scope; see Non-Goals.
- **User-visible reclamation.** A user who applies retention and then runs
  "Compact database" sees the reported before/after footprint drop by a
  measurable, explainable amount, and the settings surface states what was
  removed (row counts per table, cutoff date) before or after the operation.
  This is the claim 013's S5 could not honestly make.
- **Reference reduction target.** At the shipped 90-day default, on the
  measured corpus, retention removes ~2026-03 through ~2026-04 data: ≈524k
  `session_events` rows (~33%) and ≈166k `tool_actions` rows (~42%, ~520 MB of
  payload). Combined with the Phase 1 index drop that is ≈1.9 GB off a 7.54 GB
  file. These are *observations from one corpus*, not the acceptance
  threshold — acceptance is measured against a frozen synthetic fixture
  (see S1).
- **No silent breakage of readers.** Every consumer listed under Constraints
  either keeps working unchanged, or degrades in a defined, documented,
  user-visible way (e.g. "code insights before <date> are unavailable")
  instead of silently reporting zeros or shrinking totals.
- **Pruning is durable.** A pruned row is not resurrected by the *normal*
  transcript reconciliation sweep (an `--resume` append to an old JSONL is
  enough to re-drive a full snapshot replace), nor by a re-armed reingest
  marker or a migration that forces a reparse. Durability is achieved by an
  insert-time retention watermark, not by a one-shot DELETE (Clarifications
  Q2). Retention that silently undoes itself is worse than no retention,
  because the user believes they reclaimed space.
- **Bounded write-lock impact.** Deletion never holds the primary connection
  long enough to stall the UI or reject an ingest write: work is chunked, and
  either runs under the existing quiesce lease or is interruptible between
  chunks.
- **Composes with compaction.** Retention explicitly does not claim to free
  filesystem bytes on its own. It hands off to the existing
  `compact_database` path, and the UI never implies otherwise.

## Non-Goals

- **Not a general data-lifecycle framework.** `observations` retention
  (`cleanup_old_observations`) and token/usage hourly aggregation
  (`aggregate_and_cleanup`) already exist and are not being redesigned or
  unified here.
- **Not retention for other tables.** `model_usage_observations` (563 MB base
  + ~510 MB indexes) is the third-largest consumer and a legitimate future
  target, but it has its own backfill/revision/cursor semantics and is out of
  scope. Same for `observations` (302.9 MB), `usage_snapshots`, and
  `token_snapshots`. The three sibling owned tables (`response_times`,
  `skill_usages`, `hook_invocations`) are explicitly **kept at full history**:
  the retention watermark does not filter their inserts and no DELETE touches
  them (Clarifications Q2).
- **Not a change to what gets ingested — except for the Phase 1 payload
  write-policy decision.** Whether `tool_detail` rows should be written
  without `full_input`/`full_output` (or written at all) is decided in Phase 1
  because it competes for the same GB; changing the *rest* of what the
  extractors emit, and retroactively NULLing payloads on existing rows, both
  stay out of scope. Retroactive payload eviction is ruled out on its own
  merits: `full_input IS NOT NULL` is a load-bearing predicate in all three
  code-stats readers (see Spec Review).
- **Not rollup aggregates.** Pre-cutoff `tool_actions`/`session_events` are
  not rolled up into surviving daily or per-session aggregates. S4 ships the
  cheap degradation treatment instead. Aggregates are a deferred follow-up
  feature with their own table, migration, and five read-path changes
  (Clarifications Q5) — file as a separate bead, do not re-litigate here.
- **Not export or archive of pruned rows.** No JSONL/Parquet sidecar, no
  `*_archive` table. The pre-run consent preview is the safeguard. Export is a
  follow-up (Clarifications Q3).
- **Not pruning of live/remote rows.** Rows with `source_key IS NULL` —
  written by `store_live_session_analytics` and
  `server.rs::persist_remote_session_analytics` for sessions whose JSONL lives
  on another machine — are excluded from every DELETE and from the watermark
  filter. They are the genuinely unrecoverable class.
- **Not `source_key` normalization.** Replacing the 244-char `source_key` with
  an integer FK is rejected for now as too costly: a ~2.0M-row, two-table,
  17-index rebuild inside a startup migration with no quiesce, progress, or
  resumability (Spec Review; Clarifications Q1).
- **Not `dbstat` per-table footprint reporting.** Reporting is whole-file
  before/after bytes. `dbstat` is available but its full-file page walk is not
  paid for in MVP (Clarifications Q7).
- **Not a VACUUM/compaction redesign.** `compact_database`,
  `begin_ingest_quiesce`, the preflight, and the progress events ship already
  and are consumed, not modified — beyond any new call site or progress phase
  retention needs.
- **Not a Tantivy/session-search retention policy.** The full-text index at
  `~/.local/share/com.quilltoolkit.app/session-index/` has its own lifecycle
  and is not sized or pruned here.
- **Not transcript deletion.** Quill never deletes the user's
  `~/.claude/projects/` or `~/.codex/sessions/` JSONL files. (Claude's own
  `cleanupPeriodDays` does, independently — see Constraints.)
- **Not automatic.** Retention is strictly opt-in and defaults to "never
  prune". Nothing is ever deleted unprompted — no schedule, no idle sweep, no
  first-launch migration that prunes. The trigger is a manual control in
  Performance settings (Clarifications Q3, Q4).

## User Stories

### S1: Bounded database size under a retention window

As a long-running Quill user, I want old raw analytics rows to age out on a
policy I can see, so that the app's database stops growing without limit.

**Acceptance criteria:**

- The retention policy is **age-based and row-scoped**: "rows whose
  `timestamp` is older than N days", applied to `tool_actions` and
  `session_events` only. Rows with `source_key IS NULL` are excluded. Sessions
  that straddle the cutoff may be partially pruned; that is accepted and
  handled by S4's degradation treatment.
- The window is user-configurable from a preset list with an explicit "never"
  option. **"never" is the default state on every existing and new database**;
  90 days is the recommended preset a user must actively choose.
- **Acceptance is measured on a frozen synthetic fixture**, built in the
  `vacuum_spike.rs` pattern with known per-month row counts, so the numbers do
  not drift with one developer's machine. The fixture assertion is exact: for
  a fixture with a known count of pre-cutoff rows, the run deletes exactly
  that many and no others.
- *Observed on the measured corpus (2026-07-24, not an acceptance
  threshold):* a 90-day window covers ≈524k `session_events` rows and ≈166k
  `tool_actions` rows.
- Re-applying the policy immediately afterwards deletes 0 rows and returns a
  skipped result with a reason (idempotent).
- The cutoff is enforced **at insert time as well as by DELETE.** A global
  retention watermark stored in the `settings` table is consulted by
  `replace_transcript_analytics_snapshot`, which filters its `tool_actions`
  and `session_events` inserts against it. No schema migration, no `v35`, no
  `SCHEMA_TOO_NEW` lockout for older builds.
- The cutoff comparison is correct for the stored timestamp format. Both
  tables store `TEXT` timestamps that on the measured corpus are uniformly
  24 characters, `YYYY-MM-DDTHH:MM:SS.sssZ` (verified: 396,241/396,241 and
  1,598,902/1,598,902 rows). A build that encounters a non-conforming value
  must not silently mis-compare it — either the comparison is
  format-independent or non-conforming rows are retained and reported.
- Edge states are defined and tested: a fresh install, or any database with
  nothing older than the cutoff, previews 0 rows and runs as a no-op skip
  result; a database where *everything* is older than the cutoff is allowed to
  run, but the preview makes the total loss explicit before the user confirms.

### S2: Disk space actually comes back

As a user who just pruned, I want the file on disk to get smaller, so that
"retention" means something on my filesystem and not just in a row count.

**Acceptance criteria:**

- The UI states plainly that deletion alone frees no filesystem bytes and that
  compaction is required, either by chaining into `compact_database` or by
  surfacing a "reclaimable space pending compaction" figure.
- After retention + a successful `compact_database`, the reported before/after
  footprint is **whole-file bytes** (the same figure the existing compaction
  control reports). It therefore already includes the index pages the dropped
  rows occupied, which are the majority for `session_events`. Per-table
  `dbstat` accounting is not required for MVP (Clarifications Q7).
- If the free-disk preflight fails, retention's own result is still reported
  correctly: rows were deleted, bytes were not yet reclaimed.

### S3: Pruning does not stall or break the running app

As a user with the app open, I want pruning to run without freezing the UI or
losing an incoming hook write, so that maintenance is never a data-loss event.

**Acceptance criteria:**

- **The whole run holds `begin_ingest_quiesce`** — preview, delete phase, and
  VACUUM under one lease, matching `compact_database`. HTTP ingest returns
  retriable `503` and app-owned mutations queue, per the existing gate; hooks
  retry. Per-chunk lease acquire/release is rejected: it flaps
  `MAINTENANCE_IN_PROGRESS` and opens windows for the 120 s transcript-rescan
  loop to re-ingest mid-prune. The `write_arriving_during_quiesce_lands_after_
  unquiesce` invariant must continue to hold.
- **Deletion is chunked inside the lease**, using
  `DELETE FROM t WHERE rowid IN (SELECT rowid FROM t WHERE timestamp < ?
  LIMIT ?)`. `SQLITE_ENABLE_UPDATE_DELETE_LIMIT` is not compiled, so
  `DELETE … LIMIT` is unavailable and the `rowid IN` form is mandatory.
  Chunking bounds mutex hold time and WAL growth, not ingest availability
  (quiesce covers that).
- **A delete-phase disk/WAL preflight runs before the first chunk**, distinct
  from and in addition to the existing VACUUM 2×-file preflight. The WAL must
  hold every dirty page produced by the chunked deletes across seven
  `session_events` indexes; the VACUUM preflight does not cover this. Failing
  it aborts before any row is removed.
- **Numeric budgets — chunk size, per-chunk mutex hold, total wall time —
  are fixed by a timing spike before implementation commits to them.** See
  Clarifications Q6 and OQ13; the spike is a required prerequisite task, not
  an optional investigation.
- An interrupted run (app quit, error mid-chunk) leaves the database
  consistent: completed chunks stay deleted, no partial row is written, and
  the next run resumes without special handling.
- Progress is observable for a run that deletes hundreds of thousands of rows;
  the surface does not appear hung. The always-on-top widget's staleness
  during the lease is evaluated explicitly, not only hook retry cost.

### S4: Consumers keep telling the truth

As an analytics user, I want charts and session drilldowns to stay honest
after pruning, so that I do not read a pruned range as "a quiet month".

**Acceptance criteria:**

- Each of these commands is checked against the retention cutoff and either
  unaffected or explicitly handled: `get_code_stats`,
  `get_code_stats_history`, `get_batch_session_code_stats`,
  `get_llm_runtime_stats`, `get_session_subagent_tree`,
  `get_session_breakdown` (its `subagent_count` counts `tool_actions` rows).
- **The treatment is the cheap one, and it is pinned:** charts mark or
  truncate the pre-cutoff range, and any `all` range is relabelled **"all
  retained"**. A range that extends past the cutoff (notably `all`, and the
  30d/90d windows once the retention window is short) never renders as
  legitimate zeros. **Rollup aggregates are explicitly out of scope** and
  deferred to a follow-up bead — see Non-Goals.
- 013's in-process analytics cache is not left serving pre-prune results.
  `TableVersions` fingerprints use each table's *indexed high-water marker*,
  so a pure deletion may not move the marker at all and would be caught only
  by the 45 s TTL. Retention must force invalidation (bump the version,
  clear the cache, or emit the ingest event the caches already listen for)
  rather than relying on the TTL.
- The `has_subagents` / `subagent_count` fields in `src/types.ts` degrade in a
  defined way for sessions whose `tool_actions` rows were pruned.
  `subagent_count` UNIONs `token_snapshots ∪ response_times ∪ tool_actions`,
  and only `tool_actions` is pruned, so a pruned session's count is computed
  over mixed horizons and can disagree with its drilldown. **This is an
  accepted, documented limitation** — the fix is rollup aggregates, which are
  deferred. Likewise, Tantivy/MCP session search will surface sessions whose
  SQL drilldown is now empty; that is documented, not fixed here.

### S5: Pruned data stays pruned

As a user who reclaimed 2 GB, I want it to stay reclaimed, so that the next
app update does not silently rebuild everything I deleted.

**Acceptance criteria:**

- **The normal reconciliation path is the primary threat, not an edge case.**
  `replace_transcript_analytics_snapshot` unconditionally deletes-and-reinserts
  a source's full parse whenever its mtime/size/hash changes, so an `--resume`
  append to a months-old JSONL is by itself enough to restore that source's
  entire pre-cutoff history. After a retention run, such an append must
  reinsert only post-cutoff rows.
- The same holds for a *forced* reparse — `transcript_analytics_reingest_
  pending` re-armed by a future migration (migrations 30 and 33 both did this),
  a root restamp, or a failed-source retry.
- **Mechanism: a global insert-time retention watermark in the `settings`
  table.** `replace_transcript_analytics_snapshot` reads it and filters its
  `tool_actions` and `session_events` inserts against it, inside the same
  transaction that replaces all five owned tables. The other three owned tables
  (`response_times`, `skill_usages`, `hook_invocations`) are inserted
  unfiltered and keep full history. The `transcript_analytics_sources`
  registry row is untouched by retention — the source stays registered and
  reconcilable. A per-source `retained_through` column is ruled out because
  `prune_transcript_analytics_sources_for_root` deletes the registry row the
  column would live on.
- Settings storage is deliberate: no migration, no `v35`, so a user can
  downgrade to an older build and that build simply ignores the watermark.
- Regression tests prove both paths: (a) prune, then touch a pruned source's
  JSONL so the *normal* sweep re-drives a snapshot replace, assert the pruned
  rows do not return; (b) prune, force a reparse of a pruned source, assert
  the same.

### S6: The user understands and controls the trade

As a user, I want to know what pruning costs me before it happens, so that I
am not surprised by missing history.

**Acceptance criteria:**

- **Retention is opt-in and defaults to "never".** No database — new or
  existing — prunes anything until the user selects a window and confirms a
  run. There is no automatic or scheduled trigger.
- **The control lives in Performance settings, next to the existing "Compact
  database" control**, in the Systems Pages density per `DESIGN.md`. It is a
  destructive maintenance action, not a chat-style prompt.
- **Before any destructive run, a consent preview states exactly what will be
  deleted**: **exact** row counts per table (not estimates), the cutoff date,
  and what capability is lost (which views lose which range). Exact counts are
  computed under the quiesce lease with progress reported; the full-scan
  `COUNT(*)` cost is accepted rather than traded for an approximation, because
  the preview is the only safeguard against irreversible loss (there is no
  export — see Non-Goals).
- **The run is one composite operation: prune → preflight → VACUUM, with a
  single progress stream** and one completed/skipped result, reusing the
  `compact-database-progress` / `compact-database-finished` event shape. The
  user asks to reclaim space once, not twice.
- **A durable audit record is persisted** alongside the settings watermark:
  cutoff timestamp, run timestamp, and rows removed per table. It survives
  restart and is readable by the settings surface, so "what did I delete and
  when" has an answer after the toast is gone.
- The result is reported structurally like `compact_database` does today —
  a completed/skipped outcome with a reason — rather than a fire-and-forget
  toast.

## Constraints

**Storage engine and schema.**

- Rust + `rusqlite`, `src-tauri/src/storage.rs`. SQLite in WAL mode, 5 s busy
  timeout. Schema at migration v34; `MAX_SUPPORTED_SCHEMA_VERSION` gates
  opening. Any new migration is a one-way door — a v35 database is refused by
  older builds with `SCHEMA_TOO_NEW`.
- `tool_actions` columns (migration 30 shape, plus 33's additions):
  `id INTEGER PRIMARY KEY AUTOINCREMENT`, `provider`, `source_key` (nullable),
  `action_key`, `message_id`, `session_id`, `chain_id`, `parent_chain_id`,
  `tool_name`, `category`, `file_path`, `summary`, `full_input`,
  `full_output`, `timestamp TEXT NOT NULL`, `is_sidechain`, `agent_id`,
  `parent_uuid`, `lines_added`, `lines_removed`.
- `tool_actions` indexes: `uidx_ta_owned(provider, source_key, action_key)
  WHERE source_key IS NOT NULL`, `uidx_ta_live(provider, session_id,
  action_key) WHERE source_key IS NULL`, `idx_tool_actions_provider_source`,
  `_provider_session`, `_session`, `_message`, `_file`, `_category`,
  `_provider_session_sidechain`, `_provider_session_agent`, plus two created
  at startup by `ensure_startup_indexes`: `idx_tool_actions_category_timestamp
  (category, timestamp)` and `idx_tool_actions_category_provider_session`.
  **There is no index leading with `timestamp` alone on `tool_actions`** — an
  age-based delete either seeks via `(category, timestamp)` per category or
  scans.
- `session_events` columns: `provider`, `source_key` (nullable), `event_key`,
  `session_id`, `chain_id`, `parent_chain_id`, `agent_id`, `is_sidechain`,
  `timestamp TEXT NOT NULL`, `kind`, `uuid`, `parent_uuid`. **No rowid alias
  column is declared** (no `INTEGER PRIMARY KEY`), so it is a plain rowid
  table — batching by `rowid` is available but not by an application key.
- `session_events` indexes: `uidx_se_owned(provider, source_key, event_key)
  WHERE source_key IS NOT NULL`, `uidx_se_live(provider, session_id,
  event_key) WHERE source_key IS NULL`,
  `idx_session_events_provider_source`, `idx_se_timestamp`, `idx_se_chain`,
  `idx_se_provider_session_sidechain`, `idx_se_provider_chain_timestamp`
  (migration 31), and `idx_se_timestamp_chain(timestamp, provider, chain_id,
  is_sidechain, kind, session_id)` (migration 32), which
  `ensure_startup_indexes` recreates on every open because
  `get_llm_runtime_stats` pins it with `INDEXED BY` — Quill never runs
  `ANALYZE`, so the pin cannot be allowed to fail. **Retention must not drop,
  rename, or invalidate `idx_se_timestamp_chain`.** The one index this feature
  *does* drop is `idx_session_events_provider_source`, deliberately and only
  in Phase 1, gated on the `EXPLAIN QUERY PLAN` proof; every other index on
  both tables is preserved.
- Timestamps are RFC3339 `TEXT`. On the measured corpus every row in both
  tables is exactly 24 chars ending in `Z` (UTC, millisecond precision), so
  lexicographic comparison is safe. **Caveat:** 013's timestamp-offset
  uniformity spike covered `observations`, `token_snapshots`,
  `usage_snapshots`, `token_hourly`, `usage_hourly`, and `learned_rules` —
  it did **not** cover `tool_actions` or `session_events`. The uniformity
  claim for these two tables rests on this spec's measurement of one
  developer machine.

**SQLite mechanics.**

- `DELETE` moves pages to the freelist; the file does not shrink. Only
  `VACUUM` rewrites it. `auto_vacuum=0` on this database, and switching to
  `INCREMENTAL` requires one full `VACUUM` to take effect anyway.
- A full `VACUUM` needs roughly 2× the file free on disk and took 82,464 ms
  on the 7.45 GB fixture in `src-tauri/src/bin/vacuum_spike.rs`.
- `SELECT COUNT(*)` is a full scan in SQLite, so "how many rows will this
  prune" previews are not free at these row counts (013 already flagged this
  for cache probes). The scan cost is **accepted** in exchange for exact
  preview counts, paid under the quiesce lease with progress (Clarifications
  Q7).
- `SQLITE_ENABLE_UPDATE_DELETE_LIMIT` is **not** compiled into the vendored
  `libsqlite3-sys 0.28.0` build, so `DELETE … LIMIT` is unavailable; chunked
  deletes must use the `rowid IN (SELECT … LIMIT ?)` form. `dbstat` **is**
  available (`SQLITE_ENABLE_DBSTAT_VTAB` is set unconditionally), but is not
  used for MVP reporting.
- Deleting N rows also rewrites every index entry for those rows — with seven
  indexes on `session_events`, deletion cost and WAL churn are index-dominated,
  matching where the space is.

**Concurrency and maintenance.**

- Most storage operations share one mutex-protected primary `Connection`. A
  long-running statement blocks *every* read and write IPC, not just writers
  (013 Spec Review, critical question 1).
- `begin_ingest_quiesce` (`src-tauri/src/lib.rs`) is the existing process-wide
  reader/writer gate: maintenance takes the writer side, HTTP token ingest
  returns retriable `503`, model backfill defers its next mutation, and an
  already-admitted write completes first. Pruning should reuse this rather
  than invent a second exclusion mechanism.
- `compact_database` (`src-tauri/src/lib.rs`) already implements the shape
  pruning should mirror: acquire quiesce → disk preflight → dedicated
  connection → `Storage::vacuum_database` → structured completed/skipped
  result → `compact-database-progress` / `compact-database-finished` events →
  Performance settings surface. Documented in `lat.md/data-flow.md`
  ("Database Maintenance Pipeline") and `lat.md/backend.md` ("Database
  compaction", "VACUUM maintenance spike").
- Out-of-process writers exist (hook scripts posting over HTTP, the widget),
  so any in-memory notion of "we just pruned" cannot be the only source of
  truth.

**Data ownership — the hardest constraint.**

- On the measured corpus **100% of rows in both tables are source-owned**
  (`source_key IS NOT NULL`): 396,241/396,241 and 1,598,902/1,598,902. There
  are zero live/source-less rows. So essentially every row is nominally
  rebuildable from a JSONL transcript — and therefore nominally
  re-insertable.
- `replace_transcript_analytics_snapshot` (`storage.rs:3450`) replaces all
  five owned analytics tables for a source in a single transaction.
  Reconciliation short-circuits on unchanged `mtime_ns` + size, then on a
  content hash — but **any** change past those short-circuits (an `--resume`
  append to a months-old JSONL is enough) re-drives a full delete-and-reinsert
  of that source's entire parse. Resurrection is therefore a **normal-path**
  behaviour, not only a forced-reparse edge case, which is why the cutoff is
  enforced at insert time (S5, Clarifications Q2).
- Counter-pressure, correctly scoped: for **owned** rows the SQLite row is
  never the only copy — reconciliation already deletes owned rows for sources
  missing from a completed root scan, so every *surviving* owned row is
  transcript-backed, even after Claude's `cleanupPeriodDays` removes the file.
  The genuinely unrecoverable class is **live rows (`source_key IS NULL`)**
  written by `store_live_session_analytics` /
  `server.rs::persist_remote_session_analytics` for sessions whose JSONL lives
  on another machine — zero on the measured corpus, real for multi-host and
  widget users. Those rows are excluded from pruning entirely (Non-Goals,
  Clarifications Q3).
- `transcript_analytics_sources` holds 5,569 rows; `source_key` values average
  244 chars and are duplicated across ~2.0M analytics rows and their indexes.

**Consumers that read these tables** (verified by grep over
`src-tauri/src/`; none of them are covered by 013's caches, which cover only
model, bucket-stat, and context-savings commands):

| Table | Reader (storage.rs) | IPC command | Frontend |
| --- | --- | --- | --- |
| `tool_actions` | `get_code_stats` (`category='code_change' AND timestamp >= ?`) | `get_code_stats` | `useCodeStats`, `useCodeInsights` |
| `tool_actions` | `get_code_stats_history` | `get_code_stats_history` | `useCodeStats`, `useCodeInsights` |
| `tool_actions` | `get_batch_session_code_stats` | `get_batch_session_code_stats` | `useSessionCodeStats`, `useCodexLiveData` |
| `tool_actions` | `get_session_breakdown` (`subagent_count` subquery) | `get_session_breakdown` | Sessions tab |
| `tool_actions` | `get_session_subagent_tree` (`agent_id` enumeration, `tool_call_count`) | `get_session_subagent_tree` | `useSessionSubagents` |
| `session_events` | `get_llm_runtime_stats` (sole source; `INDEXED BY idx_se_timestamp_chain`) | `get_llm_runtime_stats` | `useLlmRuntimeStats`, `useCodeInsights` |
| `session_events` | `get_session_subagent_tree` | `get_session_subagent_tree` | `useSessionSubagents` |

**Writers:** `replace_transcript_analytics_snapshot`,
`store_live_session_analytics`, `ingest_session_events`,
`store_codex_hook_observation`, `server.rs::persist_remote_session_analytics`,
`server.rs::post_session_messages`, and the session/project/host deletion
transactions plus `prune_transcript_analytics_sources_for_root`.

**MCP is not a direct consumer.** `search_history`
(`src-tauri/claude-integration/mcp/tools/search.py`) goes through the HTTP
session-search API backed by the Tantivy index at
`~/.local/share/com.quilltoolkit.app/session-index/`; no SQL read of
`tool_actions` or `session_events` exists in the MCP tools. Note that
`lat.md/backend.md` § "Session Indexing" still describes `tool_actions` as
storing data "for MCP-powered session search" — that description appears
stale and should be verified before it is used to justify a retention
decision. The practical implication is favourable: MCP session search survives
pruning because its corpus is the Tantivy index, not these tables.

**Product framing.** `PRODUCT.md` treats the desktop app as the primary
surface and `DESIGN.md`'s "Glass Cockpit" reserves green/amber/red for a
severity meter — a destructive maintenance action belongs in the Systems Pages
density alongside the existing Performance settings compaction control, not as
a chat-style prompt.

## Open Questions

Numbering is preserved from the pre-clarification draft so review artifacts and
beads that cite an OQ number still resolve. Items 1–10 are answered; see the
Clarifications section below for the decision text.

1. **Is row deletion even the right lever?** — **Resolved, see Clarifications
   Q1.** Reframed as ranked levers across two phases. Payload eviction is not
   free (`full_input IS NOT NULL` is load-bearing) and `source_key`
   normalization is rejected as too costly; the cheap non-destructive lever is
   the `idx_session_events_provider_source` drop.

2. **Policy shape: age, size, or count?** — **Resolved, see Clarifications
   Q4.** Age-based (`timestamp` older than N days).

3. **One policy or two?** — **Resolved, see Clarifications Q4 and Q2.** One
   global window, row-scoped, applied to `tool_actions` and `session_events`
   only. Half-pruned sessions are accepted and covered by S4's treatment.

4. **Is the retention window user-configurable?** — **Resolved, see
   Clarifications Q4.** Configurable presets including "never"; "never" is the
   default state, 90 days is the recommended preset.

5. **Manual, automatic, or prompted?** — **Resolved, see Clarifications Q3 and
   Q4.** Manual only, in Performance settings next to "Compact database".

6. **Do pruned raw rows need surviving aggregates?** — **Resolved, see
   Clarifications Q5.** No. Cheap degradation treatment now; aggregates are a
   deferred follow-up bead and a Non-Goal here.

7. **Archive before delete?** — **Resolved, see Clarifications Q3.** Delete
   outright; the consent preview is the safeguard. Export/archive is a
   follow-up and a Non-Goal here.

8. **How is the cutoff enforced against re-ingest?** — **Resolved, see
   Clarifications Q2.** Insert-time global watermark in `settings`.

9. **Quiesce for the whole run, or chunk-by-chunk?** — **Resolved, see
   Clarifications Q6.** Whole-run lease; chunking bounds mutex hold and WAL
   growth, not ingest availability. Per-chunk numeric budgets are set by the
   OQ13 spike.

10. **Does retention run before compaction automatically?** — **Resolved, see
    Clarifications Q4.** One composite prune → preflight → VACUUM operation
    with a single progress stream.

**Still open (non-blocking — none of these gate planning):**

11. **Does 013's analytics cache need a new invalidation channel?**
    `TableVersions` probes an indexed high-water marker, which a pure DELETE
    may not move; the 45 s TTL is the only backstop. Is bumping a version /
    emitting `transcript-analytics-updated` sufficient, or does the cache
    primitive need a "table shrank" signal? **Blast radius is currently zero**
    — neither table appears in any `CacheTable` set today — so this only
    matters for whichever commands get cached next. S4 already requires forced
    invalidation rather than TTL reliance; the *shape* of that signal is the
    remaining open detail.

12. **Should ingest write less instead?** `tool_actions` splits into
    `command` (212,728), `tool_detail` (161,021), and `code_change` (22,492).
    Only `code_change` feeds `get_code_stats`. If `tool_detail` rows are
    unread by any surface, not writing them — or writing them without
    `full_input`/`full_output` — beats pruning them later. **Scoped into Phase
    1** as the payload write-policy decision (Clarifications Q1); the answer
    itself is still to be produced, by grepping every reader of
    `full_input`/`full_output` and the `tool_detail` category. Forward-looking
    write policy only — retroactive payload eviction stays a Non-Goal.

13. **Delete-phase timing spike — REQUIRED, no longer a question.**
    Clarifications Q6 makes this a prerequisite task, not an open decision: a
    `vacuum_spike.rs`-style binary must measure delete wall time, per-chunk
    mutex hold, and WAL growth for a ~700k-row chunked delete on a
    production-size fixture. What remains open is only the *resulting
    numbers* — chunk size and total wall-time budget are set by the spike's
    output, and the implementation must not hard-code them beforehand.

14. **Is the measured corpus representative?** Every number here comes from
    one developer machine with a 3.5-month history and 100% source-owned
    rows. A user with source-less live rows (`source_key IS NULL`), a much
    longer history, or a Codex-heavy corpus may have a different ratio.
    Validation against a second real database **stays open and non-blocking**:
    acceptance is pinned to a frozen synthetic fixture (S1), so a second
    corpus would strengthen confidence in the *defaults* rather than gate the
    build.

**Not questions (tracked as tasks):**

- The `EXPLAIN QUERY PLAN` proof that snapshot-replace deletes seek via the
  partial `uidx_se_owned` before `idx_session_events_provider_source` is
  dropped is a **Phase 1 task with a pass/fail criterion**, not an open
  question. If the proof fails, the index stays and Phase 1 loses ~473 MB of
  its target — which is a finding to report, not a decision to re-open.
- This repository has **no `constitution.md`**. Proceeding without one was an
  explicit human decision; do not block planning on creating it.

## Clarifications

Seven Critical Questions raised by the Spec Review were answered by a human
gate on 2026-07-24. These decisions are binding and are reflected in the body
of this spec above; the Spec Review below is retained unedited as the record of
what was asked and why.

**Q1: One feature or two — and which mechanism should lead?**

A: Reframe 014 as **database footprint reduction with ranked levers**, kept as
one epic with phased tasks rather than split into two specs. **Phase 1 is
non-destructive:** drop `idx_session_events_provider_source` (~473 MB) after
`EXPLAIN QUERY PLAN` proves the snapshot deletes seek via the partial
`uidx_se_owned`, and settle the `full_input`/`full_output` write-policy
question. **Phase 2 is destructive:** age-based retention. `source_key`
normalization is rejected for now as an order of magnitude beyond the pruning
it replaces. Goal 1 is restated as scoped to `tool_actions` + `session_events`
only — a perfect prune of both still leaves ~5 GB of other tables, and the
spec must not imply otherwise.

**Q2: What is the durability mechanism, and what happens to the sibling
tables?**

A: An **insert-time global retention watermark stored in the `settings`
table**. No schema migration, so no `v35` and no `SCHEMA_TOO_NEW` lockout for
users who downgrade. `replace_transcript_analytics_snapshot` reads the
watermark and filters its inserts against it for `tool_actions` and
`session_events` only. The three sibling owned tables — `response_times`,
`skill_usages`, `hook_invocations` — are inserted unfiltered and keep full
history; no DELETE touches them. The `transcript_analytics_sources` registry
row survives pruning untouched, so the source stays registered and
reconcilable. The per-source `retained_through` column is ruled out because
`prune_transcript_analytics_sources_for_root` deletes the row it would live on.

**Q3: Whose history may be deleted, and with what recourse?**

A: Retention is **strictly opt-in with "never prune" as the default**, which
keeps PRODUCT.md's "search across every past agent run" promise true for any
user who does nothing. **Live rows (`source_key IS NULL`) are excluded from
pruning outright** — remote/widget-ingested sessions with no local JSONL are
the one genuinely unrecoverable class. **No export or archive in the MVP:** the
pre-run consent preview is the safeguard, and file export is recorded as an
explicit Non-Goal and follow-up.

**Q4: What is the policy shape, trigger, and edge behaviour?**

A: **Age-based cutoff** — "`timestamp` older than N days" — with
user-configurable presets, a **90-day recommended preset**, and **"never" as
the default state**. The trigger is **manual**, from a control in Performance
settings next to the existing Compact control. Prune and VACUUM ship as **one
composite operation with a single progress stream**, because the user's request
is "reclaim space", not two sequenced chores. The cutoff is **row-scoped**:
sessions straddling the boundary may be partially pruned, which is accepted and
handled by Q5's treatment. Edge states: a fresh install or a database with
nothing older than the cutoff previews 0 rows and runs as a no-op skip; a
database where everything is older is allowed to run, but the preview makes the
loss explicit first.

**Q5: How do consumers degrade?**

A: The **cheap treatment**, pinned: mark or truncate pre-cutoff ranges in
charts, and relabel any `all` range as **"all retained"**. **No rollup
aggregates now** — they are explicitly deferred to a follow-up feature/bead,
because they mean a new table, a new migration, and changes to five read paths.
The `subagent_count` mixed-horizon problem (it UNIONs `token_snapshots ∪
response_times ∪ tool_actions`, and only the last is pruned) is documented as
an **accepted limitation**, as is the case where Tantivy/MCP search surfaces a
session whose SQL drilldown is empty.

**Q6: What are the operational budgets and delete-phase safety measures?**

A: **Whole-run ingest quiesce** — hooks retry on `503`, exactly as
`compact_database` already makes them. Per-chunk lease flapping is rejected: it
churns `MAINTENANCE_IN_PROGRESS` and lets the 120 s transcript-rescan loop
re-ingest mid-prune. Deletes are **chunked via `DELETE FROM t WHERE rowid IN
(SELECT rowid FROM t WHERE timestamp < ? LIMIT ?)`**, since
`SQLITE_ENABLE_UPDATE_DELETE_LIMIT` is not compiled. A **delete-phase disk/WAL
preflight distinct from the VACUUM preflight** runs first, because the WAL must
hold every dirty page across seven `session_events` indexes and the existing
2×-file check does not cover that. A **`vacuum_spike.rs`-style timing spike is
required to fix the numeric budgets** (chunk size, wall time) *before* the
implementation commits to them.

**Q7: What does the preview report, and what is persisted?**

A: **Exact pre-run row counts**, computed under the quiesce lease with progress
reported — the full-scan cost is accepted, because the preview is the only
safeguard in a design with no export. Footprint reporting is **whole-file
before/after bytes**; `dbstat` per-table accounting is available but **not
required for MVP**. A **persisted audit record** — cutoff, run timestamp, rows
removed per table — is stored alongside the settings watermark, so the run has
an answer after the toast is gone.

## Spec Review

Six parallel review passes (requirements, gaps, ambiguity, feasibility, scope,
stakeholders) ran 2026-07-24; feasibility claims were verified against
`src-tauri/src/` and the vendored `libsqlite3-sys 0.28.0` build script. The
consensus meta-finding: the Goals and User Stories are written as if age-based
row deletion were settled while the Open Questions leave the mechanism, window,
unit, enforcement, and trigger genuinely open — and the spec straddles two
features (destructive retention vs. non-destructive storage/index reduction).

### Code-verified corrections (these change the premises of the Open Questions)

- **OQ1(b) payload eviction is NOT "zero history loss".** `full_input IS NOT
  NULL` is a load-bearing WHERE predicate in `get_code_stats`
  (storage.rs:15872), `get_code_stats_history` (:15978), and
  `get_batch_session_code_stats` (:16081), and `get_code_stats` reads
  `full_input` as a non-optional `String`. NULLing payloads silently drops
  those rows from every code-stats surface unless the queries change too.
- **Resurrection happens on the NORMAL reconciliation path, not only forced
  reparse.** `replace_transcript_analytics_snapshot` (storage.rs:3450)
  unconditionally deletes-and-reinserts a source's full parse whenever
  mtime/size/hash change — e.g. any `--resume` append to an old JSONL restores
  that source's entire pre-cutoff history. S5's first acceptance criterion is
  wrong as written; insert-time cutoff enforcement is effectively the *only*
  viable OQ8 candidate.
- **OQ8's per-source `retained_through` column is ruled out.**
  `prune_transcript_analytics_sources_for_root` (storage.rs:3292) deletes the
  very registry row that column would live on when a source vanishes from a
  completed root scan.
- **"JSONL gone → SQLite row is the only copy" is wrong for owned rows** —
  reconciliation already deletes owned rows for sources missing from a
  completed root scan, so every *surviving* owned row is transcript-backed.
  The genuinely unrecoverable class is live rows (`source_key IS NULL`)
  written by `store_live_session_analytics` /
  `server.rs::persist_remote_session_analytics` for sessions whose JSONL
  lives on another machine — zero on the measured corpus, real for
  multi-host/widget users.
- **OQ1(c) `source_key` normalization is an order of magnitude beyond the
  pruning it replaces**: a ~2.0M-row, two-table, 17-index rebuild inside a
  startup migration with no quiesce, progress, or resumability (migration 30
  is the precedent — it had to strand `tool_actions_legacy_v30`). The one
  genuinely cheap lever inside (c) is `DROP INDEX
  idx_session_events_provider_source` (~473 MB), pending `EXPLAIN QUERY PLAN`
  proof that the snapshot deletes use the partial `uidx_se_owned`.
- **`dbstat` IS available** — `libsqlite3-sys 0.28.0` compiles the bundled
  SQLite with `SQLITE_ENABLE_DBSTAT_VTAB` unconditionally (verified in its
  `build.rs`), so S2's per-table accounting is implementable; the open concern
  is only its full-file page-walk cost. `SQLITE_ENABLE_UPDATE_DELETE_LIMIT`
  is NOT compiled, so chunked deletes must use
  `DELETE FROM t WHERE rowid IN (SELECT rowid … LIMIT ?)`.
- **013's cache blast radius is currently zero** — neither `tool_actions` nor
  `session_events` appears in any `CacheTable` set today; OQ11 only matters
  for whichever commands get cached next.

### Critical Questions (answer before planning)

1. **One feature or two — and which mechanism?** Split non-destructive
   storage/index reduction (index drop, payload policy, normalization) into
   its own spec sequenced first and keep 014 as narrow age-based retention, or
   reframe 014 as "database footprint reduction" with ranked levers? The
   corrected costs above (payload eviction not free, normalization huge,
   index drop cheap) reorder OQ1's menu; Goal 1's "bounded steady-state size"
   must also be restated as scoped to these two tables (a perfect prune still
   leaves ~5 GB of other growing tables). — flagged by: all six dimensions.
2. **Durability mechanism.** Given resurrection on the normal path and the
   ruled-out per-source column, is an insert-time global retention watermark
   (stored in `settings`, no schema migration, preserves build downgrade) the
   accepted S5 mechanism? And what is the defined post-prune state of the
   three sibling owned tables (`response_times`, `skill_usages`,
   `hook_invocations`) and the `transcript_analytics_sources` row? — flagged
   by: feasibility, gaps, ambiguity, requirements, scope.
3. **Whose history may be deleted, and with what recourse?** PRODUCT.md
   promises "search across every past agent run" — what stance does the
   default policy take? Are unrecoverable live/remote rows excluded from
   pruning outright? Is archive/export an opt-in requirement or an explicit
   Non-Goal? Is "never prune" a first-class setting? — flagged by:
   stakeholders, requirements, scope.
4. **Policy shape.** Age vs size vs count; unit = row, session, or source
   (half-pruned sessions are visible bugs in drilldowns); fixed vs
   configurable window and its default; manual vs prompted vs automatic
   trigger; composite prune→VACUUM or two controls; retention floor and edge
   states (fresh install, everything-older-than-cutoff, future-dated rows).
   — flagged by: ambiguity, scope, stakeholders, gaps.
5. **Consumer degradation semantics.** Pin S4 to the cheap treatment (mark or
   truncate pre-cutoff ranges; relabel `all` as "all retained") and defer
   rollup aggregates (OQ6 — a materially larger feature) to a follow-up?
   Define the cross-table semantic: `subagent_count` UNIONs
   `token_snapshots ∪ response_times ∪ tool_actions`, so mixed horizons make
   session numbers self-contradictory; Tantivy/MCP hits will surface sessions
   whose SQL drilldown is empty. — flagged by: scope, ambiguity,
   stakeholders, gaps.
6. **Operational budgets and delete-phase safety.** Quiesce for the whole run
   vs per-chunk (per-chunk flaps `MAINTENANCE_IN_PROGRESS` 503s and opens
   windows for the 120 s transcript-rescan loop to re-ingest mid-prune);
   numeric budgets for chunk size / mutex hold / total wall time; a disk+WAL
   preflight for the DELETE phase itself (WAL must hold every dirty page —
   the existing 2× preflight only covers VACUUM); does this need its own
   `vacuum_spike.rs`-style timing spike before committing? — flagged by:
   requirements, gaps, feasibility, ambiguity, stakeholders.
7. **Preview, reporting, audit record, and migration posture.** The pre-run
   preview's `COUNT(*)` full-scans `tool_actions` under the primary mutex —
   what accuracy/latency is acceptable? Footprint reporting: whole-file bytes
   or `dbstat` per-table accounting, at what cost? Persist a durable retention
   audit record (cutoff, timestamp, rows removed per table) — the natural
   home is the same watermark artifact as Q2. Settings-based (no migration,
   downgrade-safe) vs schema-based (v35 `SCHEMA_TOO_NEW` lockout)? And what
   is the numeric success target for the feature as a whole? — flagged by:
   requirements, gaps, feasibility, stakeholders, ambiguity.

### Non-Blocking Observations

- S1's row-count thresholds are pinned to one machine on one date; restate
  acceptance against a frozen synthetic fixture (the `vacuum_spike.rs`
  pattern) and demote corpus numbers to observations.
- `lat.md/backend.md` § Session Indexing still says `tool_actions` backs
  "MCP-powered session search" — stale; MCP goes through the Tantivy index.
- The learning pipeline reads transcript JSONL directly (`learning.rs`), not
  these tables — safe today; record the expiry condition (a future learn
  pipeline over `tool_actions` would become a retention stakeholder).
- `idx_se_timestamp_chain` (pinned via `INDEXED BY`) is safe so long as
  retention drops no index; keep that invariant explicit in the plan.
- Move the defer-candidates — export/archive, composite operation,
  second-corpus validation — into Non-Goals explicitly once Q3/Q4 are
  answered, so they are not re-litigated during planning.
- S1's timestamp criterion contains an unresolved either/or (format-
  independent comparison vs retain-and-report); pick one during planning and
  say where "reported" surfaces.
- The always-on-top widget is the product's default surface; evaluate Q6's
  quiesce choice against widget staleness, not only hook retry cost.
