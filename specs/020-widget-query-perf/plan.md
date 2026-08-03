# Plan: widget-query-perf

Implementation plan for the clarified spec (all 7 Clarifications binding).
Slice cut per Q5: A = model rollup (S1), B = runtime stats (S2) first and
independent; C = readers (S3), D = refresh honesty (S4), E = query cleanups
(S5) beaded now but gated on a post-A/B re-measurement. All acceptance is
measured on the frozen 13.5 GB corpus per Q3. This plan supersedes two
recorded 013 decisions (cited in Architecture Approach).

## Architecture Approach

Replace O(rows) scans with source-keyed hourly rollups maintained in the
ingest transaction, read hybrid (closed hours from rollup, open hour from
raw), backfilled out-of-band under the quiesce lease; then, gated on
re-measurement, split reads onto reader connections and make the frontend
refresh honest.

**A — model rollup.** New `model_usage_hourly` at grain
`(hour_utc, provider, derived_model_id, source_key)` (Clarification Q1,
binding). The ingest write path that inserts `model_usage_observations`
rows (via the storage layer guarding `Mutex<Connection>` at
storage.rs:3165) folds each batch into the rollup by UPSERT **in the same
transaction** — the rollup is therefore exact at every commit and needs no
freshness queue. Reads (`get_model_usage_overview` storage.rs:7111,
uncached body :7128 with the temp-table materialization at :7201; model
history ~:8095) serve closed hours (`hour_utc < current hour`) from the
rollup and only the open hour from raw — a bounded indexed seek (worst
burst ~866k rows/day ⇒ ~36k rows/hour). Distinct facets (the
DISTINCT (provider, analytics_session_id) counts at ~:7256) decompose
because `model_observation_sources` carries exactly one
`analytics_session_id` per source (DDL storage.rs:5298-5310) and the
rollup row denormalizes it; source-level attributes (hostname, cwd,
suppression) come from a join against the small sources table.
Suppression flips (UPDATEs storage.rs:5463, 9000-9271; deletes :2256) are
honored at read time via that join, so a flip costs nothing and is exact
by construction; source deletion and re-ingest DELETE exactly that
source's rollup rows and refold from raw inside the same transaction —
satisfying Q1's "invalidate exactly that source's rollup rows" with the
suppression case strengthened to read-time filtering.

**B — runtime stats.** Redefine turn-gap semantics time-invariant
(Clarification Q2, binding): a closed turn's tool-wait gap is
`min(next_event_ts − prev_ts, TOOL_WAIT_MAX_SECS)` — a pure function of
the data; the now-clamp (storage.rs:17303-17306) applies only to the
single open trailing turn per chain. Ingest finalizes closed turns per
source as events arrive and folds them into `runtime_hourly` keyed
`(hour_utc, provider, source_key)` (turn attributed to its start hour);
`get_llm_runtime_stats` (storage.rs:17285) reads closed hours from the
rollup and computes only open turns from the raw `session_events` tail.
Re-ingest's per-source DELETE+reinsert (storage.rs:17888) deletes that
source's `runtime_hourly` rows and per-source finalization bookmark in the
same transaction and refolds.

**Backfill (A and B).** One chunked backfill framework on the writer
connection: bounded per-chunk transactions, per-chunk
`wal_checkpoint(TRUNCATE)` (retention precedent retention_engine.rs:1262),
disk preflight (pattern retention_engine.rs:321-361), resumable bookmark
persisted per chunk, and a yield check on the ingest-quiesce lease
(lib.rs:110-151) between chunks — prune/VACUUM win (Clarification Q4).
During backfill, Models keeps the existing raw path plus a "building
index" progress note; a `rebuild_model_rollup` command lands in the
settings Performance tab next to `compact_database`
(PerformanceTab.tsx:261). Fresh installs see an instant-complete backfill
(no rows to fold).

**C/D/E (gated).** C extends the `open_model_analytics_reader` pattern
(storage.rs:6216, `SQLITE_OPEN_READ_ONLY | NO_MUTEX`) to the view-serving
commands still slow after A/B and moves post-query CPU (bucketing,
downsampling) off the MutexGuard. D makes refresh honest: module-level
frontend cache (replacing the per-instance `useRef` at
useCachedInvoke.ts:75, surviving ViewRegion's unmount-on-switch at
ViewRegion.tsx:111), ≥5s ingest-event coalesce (Clarification Q6c,
normative), range×2 comparison windows (Q6d), range-scoped skills
breakdown (Q6b — bug), Trends scoped to 7d+prior-7d, and removal of the
unconditional second `useBreakdownData("projects")`
(UsageView.tsx:439-443). E fixes `get_code_stats`/`get_code_stats_history`
(storage.rs:16610/16733 `full_input` fetch), time-bounds
`get_session_breakdown` (storage.rs:11222), and runs a bounded ANALYZE.

### Alternatives rejected

- **SQL window-function rewrite for S2 — rejected.** It still walks every
  `session_events` row in the window (~2.9M at 90d) through the covering
  index; the spec review's estimate is 2-5× improvement, not the 10×
  needed for ≤200ms @90d. It also cannot express the now-dependent
  tool-wait ceiling without the same semantics redefinition the rollup
  needs — so it pays the semantic cost without meeting the budget.
  Clarification Q2 records the rejection as binding.
- **Dirty-hour queue vs source-keyed rollup — queue loses.** Suppression
  flips, source deletes, and re-ingest each invalidate arbitrary
  per-source hour sets; a dirty-hour queue is a second stateful mechanism
  with its own crash-consistency story and still cannot serve the
  DISTINCT session facets (hour-keyed sums don't decompose). Source-keyed
  rows make invalidation a transactional `DELETE ... WHERE provider=? AND
  source_key=?` plus refold, colocated with the mutation, and give the
  session facet dimension for free.
- **token_hourly's delete-raw-after-fold pattern — does not transfer.**
  `aggregate_and_cleanup_tokens` (storage.rs:15618) folds >30d snapshots
  then deletes the raw rows. Here raw `model_usage_observations` must
  remain: per-token_count fidelity is the intentional "replayable
  evidence" contract (lat.md/data-flow.md:131-155, Clarification Q6a),
  and pruning is owned by retention (feature 014), not the rollup. Also
  the hot window IS the last 30 days (96% of rows), so there is no
  cold-data-only region to fold-and-delete. Hence: raw stays, hybrid
  read with an open-hour boundary, and retention's prune becomes the only
  path that removes raw (with fold-then-prune ordering, see Data Model).

### Superseded 013 decisions

- **Live-refresh-on-ingest (013 Clarification Q7 / plan.md "TTL vs. live
  Now tab" risk: "live ingest events forcing an immediate [refresh]")** —
  superseded by Clarification Q6c: during active ingest a mounted view's
  fan-out coalesces to ≥5s. Recorded as normative in the spec.
- **Temp-table-over-CTE (013 timing-measurement.md, "Temp-table versus
  CTE follow-up": CTE rejected, temp table retained at 5.8s cold)** —
  superseded structurally: the rollup removes the 4M-row materialization
  entirely for closed hours instead of re-litigating temp-table vs CTE.
  The temp-table code path is retained only as the open-hour/backfill
  fallback and shrinks to ≤1 hour of rows.

### Plan-time verification results (2026-08-02, worktree)

- **RFC3339-TEXT lexicographic ordering** — safe. Writers serialize via
  chrono `to_rfc3339()` / millis→RFC3339 helpers (uniform `+00:00`;
  storage.rs:5964, model_usage.rs helpers), and the 013 spike ("Verify
  timestamp offset uniformity", 013 plan.md Sequencing, passed
  2026-07-24) confirmed every stored RFC3339 value uses `+00:00`. The
  load-bearing comparison `timestamp >= ?1` under the `INDEXED BY
  idx_se_timestamp_chain` pin (storage.rs:17370-17379) is sound.
  `model_usage_observations` uses INTEGER `observed_at_ms` — no TEXT
  ordering dependency for slice A.
- **Manage/settings surfaces calling touched commands** — none. Grep over
  `src/`: `get_model_usage_overview`, model history,
  `get_llm_runtime_stats`, `get_code_stats*`, `get_session_breakdown`
  are invoked only from widget hooks (`useModelAnalytics`,
  `useLlmRuntimeStats.ts:25`, `useCodeStats`, `useCodeInsights.ts:159`,
  `useBreakdownData.ts:88`, `useWeeklyTrends`) and widget views. Settings
  surfaces invoke only maintenance/CPA commands
  (PerformanceTab.tsx:261/272/302 — `compact_database`,
  `preview_retention`, `run_retention_maintenance`; IntegrationsTab —
  CPA); sessions and learning windows invoke none of the touched
  commands. Slice C therefore migrates widget callers only (enumerated in
  API / Interface Changes).
- **PRAGMA analysis_limit** — available. rusqlite 0.31 with `bundled`
  (src-tauri/Cargo.toml:27) ships SQLite 3.45.x; `analysis_limit` exists
  since 3.32 and `PRAGMA optimize` since 3.18. No current
  ANALYZE/analysis_limit usage in `src-tauri/src` (grep: zero hits) —
  today's planner is stat-free, as the comment at storage.rs:17360-17366
  assumes.

### Constitution check (12 principles)

1. **Local source-backed truth — tension, resolved.** Post-prune the
   rollup is the only record for a window (retention deletes
   `model_usage_observations` rows, retention.rs:158-173, and writes NO
   retention aggregate, retention_engine.rs:1200-1202). Clarification Q1
   makes the rollup authoritative for pruned windows; principle 1 is
   served by exact fold-then-prune ordering (prune refuses to delete raw
   the rollup hasn't folded), an explicit `raw_pruned` marker so
   consistency checks and rebuild-from-raw never silently regenerate or
   drop authoritative rows, and gaps staying explicit (no interpolation).
2. **Stack/boundaries — compliant.** SQLite via rusqlite, Rust storage +
   IPC, React strict-TS hooks. No new engine, no new deps.
3. **Responsive execution — tension, bounded.** Backfill of 4.2M + 2.9M
   rows is real background work: bounded per-chunk transactions
   (≤250ms target), quiesce-lease yield between chunks, per-chunk WAL
   checkpoint, progress events, never on startup path. ANALYZE bounded
   via `analysis_limit` and run from the maintenance path, not startup.
4. **Recoverable mutation — compliant.** Fold shares the ingest
   transaction; backfill bookmark persists per chunk (resumable);
   invalidation+refold is transactional; writer stays serialized.
5. **Typed failure boundaries — compliant.** Backfill/rebuild failures
   surface as typed progress-event error states in the Performance tab,
   not silent `log::warn`; query errors propagate as today.
6. **Zero-warning gates — compliant.** fmt/lint/typecheck/build/tests
   green per item.
7. **Authorized testing — compliant.** Three families authorized at
   clarify (Q7); each key test linked one-to-one with a lat.md spec.
8. **Traceability — compliant.** lat.md updates (data-flow, features,
   frontend, test specs) and `lat check` are the final gating item.
9. **Glass Cockpit — compliant.** UI deltas are minimal: a Models
   "building index" note (widget — Flat Polish rules apply) and a rebuild
   row in the settings Performance tab (stated migration exception,
   DESIGN.md §6). No visual redesign (spec Non-Goal).
10. **Measured performance — compliant, with recorded caveat.** Explicit
    budget table (Testing Strategy), frozen 13.5 GB corpus, committed
    timing doc in 013 style. Caveat per Q3: OS page cache uncontrolled
    but recorded; budgets are machine-relative to the dev machine.
11. **Explicit external transmission — compliant.** Nothing leaves the
    device.
12. **Gated delivery — compliant.** Sequencing becomes the bead DAG under
    a new epic; commit/push only with explicit authority.

## Affected Components

Backend (`src-tauri/src/`):

- **storage.rs**
  - Migration 36→37 appended after the v36 block (~:4770+ migration
    region): additive DDL only (tables + indexes below);
    `MAX_SUPPORTED_SCHEMA_VERSION` 36→37 (storage.rs:101);
    `SCHEMA_TOO_NEW` refusal (:4130-4133) is the accepted downgrade UX.
  - Ingest observation write path: fold-into-`model_usage_hourly` UPSERT
    in the same transaction; bump `rollup_generation`.
  - Session-event ingest path: per-source turn finalization + fold into
    `runtime_hourly`; per-source bookmark in `runtime_turn_state`.
  - `get_model_usage_overview` (:7111/:7128, temp table :7201) and model
    history (~:8095): hybrid read — closed hours from rollup + sources
    join, open hour from raw; temp-table path retained for
    backfill-in-progress fallback and open-hour scope only.
  - Distinct-count facets (~:7256): rewritten over rollup rows
    (denormalized `analytics_session_id`) + sources join.
  - `get_llm_runtime_stats` (:17285-17470): closed hours from
    `runtime_hourly`; open-turn tail from raw with the existing
    `INDEXED BY` pin (:17370) narrowed to the tail window; now-clamp
    (:17303-17306) applies to open turns only.
  - Suppression UPDATE sites (:5463, 9000-9271) and source delete
    (:2256): no rollup writes needed (read-time join); re-ingest
    DELETE+reinsert (:17888): add same-transaction rollup DELETE+refold
    for both rollups.
  - 013 cache probe layer (:203-318): add `rollup_generation` to the
    probed fingerprint for model/runtime commands (high-water MAX probes
    are blind to UPSERTs).
  - Slice E: `get_code_stats` (:16610) / `get_code_stats_history`
    (:16733) stop selecting `full_input` when migration-33 line counts
    exist; `get_session_breakdown` (:11222) subqueries time-bounded /
    restructured so LIMIT 200 prunes work.
- **retention.rs / retention_engine.rs**: prune of
  `model_usage_observations` (retention.rs:158-173) gains the
  fold-then-prune precondition and sets `raw_pruned` on covered rollup
  rows (engine writes no retention aggregate for model observations —
  retention_engine.rs:1200-1202 — unchanged).
- **lib.rs**: register `rebuild_model_rollup`; backfill task spawned like
  the hourly aggregator task (lib.rs:5633-5654), honoring the quiesce
  lease (:110-151); commands stay on `block_in_place` (:1658). Slice C:
  reader-connection plumbing for migrated commands.
- **model_usage.rs**: no parser changes (Q6a — rollup is the compression
  point); the Codex token_count granularity (~:1278/:1575) and oversize
  rejection (:3055-3077) are unchanged inputs to sizing.

Frontend (`src/`):

- **hooks/useCachedInvoke.ts**: per-instance `useRef` cache (:75) →
  module-level keyed cache with TTL + stale-while-revalidate; ingest
  event coalesce raised to ≥5s (D).
- **hooks/useCodeInsights.ts**: comparison windows :52-65 change from
  next-larger-preset to prior-period range×2 (D).
- **hooks/useWeeklyTrends.ts**: fixed 30d scan → 7d + prior 7d (D).
- **components/widget/views/UsageView.tsx**: `SKILL_ALL_TIME` (:84) →
  selected range; unconditional second `useBreakdownData("projects")`
  (:439-443) made lazy/conditional (D).
- **components/widget/ViewRegion.tsx** (:111): unchanged code; the
  module-level cache makes unmount-on-switch cheap (D).
- **components/settings/PerformanceTab.tsx**: rebuild-rollup row +
  progress/error states next to Compact database (:261) (A).
- **components/widget/views/ModelsView.tsx**: "building index" note
  while backfill incomplete (A).

Docs: `lat.md/data-flow.md` (rollup fold, hybrid read, fold-then-prune),
`lat.md/features.md`, `lat.md/frontend.md`, new test-spec sections.

## Data Model

Migration 37 is **additive DDL only**; backfill runs out-of-band (013
migration-34 precedent: schema step stays fast, never blocks startup).

**`model_usage_hourly`** — grain per Clarification Q1:

- Key: `hour_utc INTEGER` (epoch-ms floored to the UTC hour — matches
  `observed_at_ms`; hourly, never daily, so no timezone is baked in),
  `provider TEXT`, `derived_model_id TEXT` (normalized model identity the
  overview groups by; the `model_evidence='missing'/'invalid'` bucket is
  a distinct derived id so gaps stay explicit), `source_key TEXT`.
  `UNIQUE(hour_utc, provider, derived_model_id, source_key)`.
- Denormalized dimension: `analytics_session_id TEXT` (1:1 per source,
  model_observation_sources DDL storage.rs:5298-5310) — makes DISTINCT
  session counts a small-cardinality aggregate over rollup rows.
- Measures: `obs_count`, `turn_count` (observation_kind='turn'),
  `token_count` (kind='token'), `sidechain_count`, and the four token
  sums `input_tokens`, `output_tokens`, `cache_creation_tokens`,
  `cache_read_tokens` (NULL-aware: separate `*_present` counts so NULL ≠
  0 — principle 1, no invented data), plus `first_observed_at_ms` /
  `last_observed_at_ms` for reach/running-now facets.
- Flags: `raw_pruned INTEGER NOT NULL DEFAULT 0` — set when retention
  deletes the underlying raw rows; a `raw_pruned=1` row is authoritative
  (rebuild-from-raw must preserve it; consistency tests skip it).
- Indexes: the unique key (hour-leading — serves all range reads) plus
  `(provider, source_key)` for invalidation deletes. Both covering for
  their access paths.
- **Size at the 1M rows/day burst envelope (Q6a: size for bursts, not
  the 135k/day average):** rollup rows/day = active sources × active
  hours × models/hour. Burst evidence: 866k raw rows/day from as few as
  77 files; envelope 200 sources × 12 h × 2 models ≈ **≤5k rollup
  rows/day** vs 1M raw — ≥200:1 compression (raw is ~745 rows/MB).
  Post-`quill-xnb` revalidation of the canonical bounded corpus folds
  4.10M observations into 7,288 rows and 7.44 MB versus 5.98 GB for the
  raw table plus indexes (804:1 physical compression). Actual rollup
  footprint is ~1,021 B/row including both indexes, not the prior 350 B
  estimate; 1.8M constant-burst annual rows therefore project to
  ~1.84 GB, not ~650 MB. The snapshot still exhibits the old post-07-28
  quiet-source shape, so `quill-45m.27` owns a fully reconciled corpus,
  corrected-volume rerun, and final annual budget.
  The existing 7 indexes / 3.6 GB on `model_usage_observations` are kept
  as-is; post-rollup index cleanup is an explicit follow-up outside this
  feature (per the spec Non-Goal).

**`runtime_hourly`** — grain `(hour_utc INTEGER, provider TEXT,
source_key TEXT)` UNIQUE; denormalized `session_id` (sessions facet is
DISTINCT (provider, session_id)); measures `turn_count`,
`runtime_secs` (sum of finalized clamped gaps), `first_turn_start_ms`,
`last_turn_end_ms`. A closed turn is attributed to its start hour; this
attribution IS the redefined windowing semantics (recorded in lat.md) and
the parity reference uses the same rule. Same row-count class as
`model_usage_hourly` (~≤5k/day burst; session_events 2.9M backfills to
~100k rows). Carries the same `raw_pruned INTEGER NOT NULL DEFAULT 0`
flag with the same fold-then-prune precondition: retention's prune of a
source's `session_events` rows requires that source's `runtime_hourly`
coverage through the pruned window (prune waits or folds synchronously),
then sets `raw_pruned=1`; pruned windows are served solely from
`raw_pruned=1` rollup rows.

**`runtime_turn_state`** — per-source finalization bookmark:
`(provider, source_key)` UNIQUE, `finalized_through_rowid INTEGER`,
`open_turn_started_ms INTEGER NULL`. Deleted+rebuilt with the source on
re-ingest (storage.rs:17888) in the same transaction. Open-turn queries
scan only rows past the bookmark. When retention prunes rows at or below
`finalized_through_rowid`, the bookmark clamps forward (never behind the
prune horizon) — pruned windows are already folded per the fold-then-
prune precondition and are served solely from `runtime_hourly`
`raw_pruned=1` rows.

**`rollup_meta`** — single-row table: `rollup_generation INTEGER`
(monotonic, bumped in every transaction that writes either rollup — the
cache-visibility marker for the 013 probe layer, which is MAX-only and
structurally blind to UPSERTs, storage.rs:203-318),
`model_backfill_done_through_ms INTEGER NULL`,
`runtime_backfill_done_through_rowid INTEGER NULL` (resumable bookmarks;
NULL = backfill complete/not started per an accompanying status column).
Crash consistency: bookmark and chunk fold commit in one transaction —
an interrupted backfill resumes exactly at the bookmark (principle 4).

**Rollup vs retention (binding semantics):** raw rows remain the source
of truth **until pruned**; retention's prune of a source's
`model_usage_observations` rows requires that source's rollup coverage
through the pruned window (fold-then-prune ordering — prune waits or
folds synchronously), then sets `raw_pruned=1`. After that the rollup is
the authoritative record for the window (Clarification Q1 supersedes
"raw is source of truth" there). The same semantics apply to
`runtime_hourly` and the `session_events` prune. The rebuild command
(either target) rebuilds only `raw_pruned=0` rows and refuses to touch
pruned coverage. `retention_daily_aggregates` interplay: for model
observations the engine deliberately writes none
(retention_engine.rs:1200-1202), so no double-counting is possible;
unlike model observations, the engine DOES write
`retention_daily_aggregates` for session_events, so `runtime_hourly`
reads for pruned windows must not double-count against them —
`runtime_hourly` wins for runtime stats, and
`retention_daily_aggregates` remain authoritative for token/code totals
only.

**Migration one-way door:** 36→37 stamps the DB; older builds refuse via
`SCHEMA_TOO_NEW` (storage.rs:4130). Accepted (013 precedent); additive
DDL means data is intact on refusal. Documented in the migration comment
and release notes.

## API / Interface Changes

Behavior changes, **no signature changes** (same args, same response
shapes — callers unaffected):

- `get_model_usage_overview`, model history — hybrid rollup read; during
  backfill they serve the existing raw path and set a new optional
  `building_index: bool` field on the response (additive, tolerated by
  strict TS as optional) driving the Models note.
- `get_llm_runtime_stats` — aggregated path + open-turn tail; closed-turn
  values become time-invariant (semantics change documented in lat.md;
  numbers for closed turns will differ slightly from the old
  now-clamped walk — this is the Q2-approved redefinition, not a
  regression).
- Slice E: `get_code_stats`, `get_code_stats_history`,
  `get_session_breakdown` — same shapes, cheaper plans.

New command + events:

- **`rebuild_model_rollup`** (registered in `tauri::generate_handler!`):
  takes a `target` argument (`model` | `runtime`); drops that rollup's
  `raw_pruned=0` rows, re-runs the chunked backfill, returns a
  structured started/refused result (refused while retention or
  compaction holds the quiesce lease). Either target preserves
  `raw_pruned=1` coverage — rebuild-from-raw never regenerates or drops
  pruned-window rows (Data Model semantics). Progress via
  `rollup-backfill-progress` events `{ phase, rows_done, rows_total,
  hour_done_through }` and a terminal `rollup-backfill-finished`
  `{ status: "completed" | "interrupted" | "error", detail? }` — consumed
  by PerformanceTab (alongside compact_database at :261) and by the
  ModelsView note. The automatic first-run backfill emits the same
  events.

Frontend hook/API changes (slice D):

- `useCachedInvoke.ts` — module-level cache keyed
  `(command, serialized args)` with TTL + stale-while-revalidate; event
  coalesce constant ≥5000ms (supersedes the 1s coalesce; normative per
  Q6c). Hook return shape unchanged.
- `useCodeInsights.ts:52-65` — `comparisonRange` = prior period of equal
  length (two windows: `[now−2R, now−R)` and `[now−R, now]`), replacing
  next-larger-preset; total scanned window exactly range×2, the single
  permitted over-range query (Q6d).
- `useWeeklyTrends` — 7d + prior 7d (same range×2 rule) instead of fixed
  30d.
- `UsageView.tsx:84` — skills breakdown takes the selected range (bug
  fix, Q6b); `:439-443` — second `useBreakdownData("projects")` becomes
  lazy (mounted only when its section renders).

**Slice C reader migration — explicit caller enumeration.** Satisfying
S3 requires ALL view-serving reads off the writer connection; the
post-A/B re-measurement decides migration *priority and effort sizing*,
not whether a path migrates. Migrate to read-only reader connections
(pattern storage.rs:6216): `get_llm_runtime_stats` (open-turn tail;
callers useLlmRuntimeStats.ts:25, useCodeInsights.ts:159/165),
`get_code_stats` / `get_code_stats_history` (useCodeStats,
useCodeInsights), `get_session_breakdown` + breakdown commands
(useBreakdownData.ts:88), weekly-trends command (useWeeklyTrends), and
the 013 cached-only endpoints `get_all_bucket_stats` /
`get_context_savings_analytics` (cold cost unmeasured at 13.5 GB —
measured in the re-measurement item to size and order the migration).
Models overview/history already run on their own reader. **Out of
scope:** all Manage/settings surfaces — verified none call the touched
commands (PerformanceTab: compact_database/preview_retention/
run_retention_maintenance stay on the maintenance path; IntegrationsTab:
CPA only; sessions and learning windows: none). Readers are short-lived
per query (open→query→close, as the model reader does) to avoid WAL
checkpoint starvation and to preserve compaction's close-readers
contract.

## Testing Strategy

Authorized at the clarify gate (Q7, constitution 7): three families plus
the S2 parity harness. Each key test carries exactly one `// @lat:` ref
to a new spec section (constitution 7/8); new sections live in a
`lat.md` test-spec file with `require-code-mention: true`.

**Family 1 — rollup consistency** (Rust, `src-tauri/tests/` + inline):
- Closed-bucket exactness: on a seeded DB, overview/history/facets from
  the hybrid path equal the raw-row computation integer-for-integer for
  every closed hour; open-hour tail equals raw by construction. Also run
  once against the frozen corpus via the harness (S1 AC).
- Source-keyed invalidation: suppression flip changes results instantly
  (read-time join); source delete and re-ingest DELETE+reinsert
  (storage.rs:17888) leave rollup == raw refold; NULL token evidence
  never becomes 0.
- Fold-then-prune: prune with incomplete rollup coverage is refused;
  after a correct prune, `raw_pruned=1` rows persist, consistency check
  skips them, and the rebuild command preserves them. Same checks for
  `runtime_hourly` on a session_events prune, plus: the
  `runtime_turn_state` bookmark clamps forward past pruned rowids, and
  runtime stats for pruned windows come solely from `runtime_hourly`
  with no double-count against `retention_daily_aggregates` (which
  remain for token/code totals only).
- Crash consistency: interrupt backfill mid-chunk (transaction abort) →
  bookmark and rollup agree; resume completes to exactness.

**Family 2 — backfill/quiesce concurrency** (Rust integration):
- Backfill yields the quiesce lease (lib.rs:110-151): a lease request
  (retention/VACUUM) acquires within one chunk bound; backfill resumes
  after release from its bookmark.
- Ingest during backfill is never blocked beyond a chunk transaction and
  never dropped; per-chunk `wal_checkpoint(TRUNCATE)` keeps WAL bounded
  (assert WAL size stays under a fixed multiple of chunk size).
- Disk preflight refuses to start under the free-space floor.

**Family 3 — 5s-injection contention** (Rust integration, slice C):
- With an artificial 5s query injected on one path, fast-class queries
  complete ≤100ms p95 under concurrent ingest writes; zero
  `database is locked` / busy-timeout errors (S3 AC).

**S2 parity on frozen corpus with pinned `now`:** a reference
implementation of the redefined time-invariant semantics (closed turns
pure f(data); open turns f(data, now)) runs against the frozen corpus
with a pinned `now`; the aggregated path must match exactly at 24h/30d/
90d, and closed-turn values must be identical across repeated runs.

**Benchmark harness + frozen-corpus protocol (acceptance evidence,
013 timing-measurement.md style):** a spike-bin style read-only harness
(house precedent: `eqp_index_drop_spike.rs`, `retention_spike.rs`)
against a frozen snapshot copy of the live 13.5 GB `usage.db` (~14 GB
disk, location recorded), pinned window endpoints, "cold" = first
in-process call with app caches bypassed (OS page cache uncontrolled,
recorded), each query at 24h/30d/90d before/after, results committed as
`specs/020-widget-query-perf/timing-measurement.md`.

Budget table (per-query unless noted; frozen corpus; machine-relative):

| Measure | Budget |
| --- | --- |
| `get_model_usage_overview` cold, 30d and 90d | ≤500ms |
| Model history cold, 30d and 90d | ≤500ms |
| `get_llm_runtime_stats` cold @90d | ≤200ms |
| `get_session_breakdown` @30d (E) | ≤300ms |
| `get_code_stats_history` @30d (E) | ≤300ms |
| Fast-class queries under 5s injection (C) | ≤100ms p95 |
| Usage / Charts / Context cold view render @30d (per-view fan-out) | ≤1200ms each |
| Warm path | no slower than current warm (013 cache regression guard) |
| Ingest fold overhead (A+B, per ingest transaction) | ≤10% added p95 latency |
| Backfill chunk transaction | ≤250ms target; lease yield between chunks |

Frontend: no test runner exists (013 decision stands); D items gate on
`tsc --noEmit` + eslint + manual verification in the dev IPC mock,
including query-window logging (dev-level log of each command's WHERE
window) to make S4's "no query exceeds R except range×2" checkable.

## Risks

- **Mid-build discoveries in facet semantics.** The overview's ~13
  result sets (facets at storage.rs:7256, reach/running-now, delegation)
  may hide a facet that doesn't decompose over the grain (e.g. a
  per-cwd or cross-source distinct). Mitigation: facet inventory is the
  first task of the hybrid-read item; any non-decomposable facet either
  gains a denormalized dimension column (schema is pre-backfill, cheap
  to extend before migration lands) or is explicitly served from the
  open-hour/raw path with a measured bound — decided before migration
  37 merges, never mid-backfill.
- **WAL growth during backfill.** Baseline WAL is already 205 MB;
  backfilling 4.2M + 2.9M rows on the writer connection can balloon it.
  Mitigation: retention's per-chunk `wal_checkpoint(TRUNCATE)`
  (retention_engine.rs:1262) after every chunk, disk preflight
  (pattern :321-361) before starting, WAL-size assertion in Family 2.
  Long-lived readers block TRUNCATE checkpoints — readers are
  short-lived per query (see slice C), and backfill pauses
  checkpoint-retry with warning rather than growing unbounded.
- **ANALYZE shifts plans app-wide.** The code today relies on a
  stat-free planner (comment storage.rs:17360-17366) and pins one query
  with `INDEXED BY` (:17370). Running ANALYZE creates `sqlite_stat1` and
  can re-plan every query in the app. Mitigation: gated in slice E after
  re-measurement; run with `PRAGMA analysis_limit=1000` (available,
  SQLite 3.45.x) from the manual maintenance path (Performance tab /
  post-rebuild), never startup; before/after EXPLAIN QUERY PLAN audit of
  every touched query on the frozen corpus; keep the `INDEXED BY` pin
  (it is immune to stats by design). Rollback: `DELETE FROM
  sqlite_stat1` restores the stat-free planner.
- **Checkpoint starvation from reader connections.** Multiple long-lived
  readers prevent WAL reset and grow the file. Mitigation:
  open→query→close readers (matching open_model_analytics_reader usage);
  compaction's close-readers contract preserved; Family 2/3 assert no
  starvation under concurrent load.
- **Rollback / one-way door.** Migration 37 stamps the DB; older builds
  hard-refuse (`SCHEMA_TOO_NEW`, storage.rs:4130). Mitigation: additive
  DDL only — no raw data is modified, so refusal never means loss; the
  migration comment + release note document it (013 precedent); rollup
  tables can be dropped and rebuilt from raw at any time except
  `raw_pruned` coverage, which is the deliberate Q1 semantics.
- **Ingest write-path latency.** Same-transaction fold adds UPSERTs
  (typically 1-5 per batch) and turn finalization adds computation to
  every session-event ingest. Budget: ≤10% added p95 per ingest
  transaction, measured by the harness on burst-shaped batches
  (~745 rows/MB Codex token_count granularity, model_usage.rs:1278/1575;
  oversize rejection :3055-3077 unchanged). If exceeded: fold moves to a
  post-commit same-connection follow-up transaction with the current
  hour always served raw (hybrid read already tolerates this).
- **Corrected-volume validation needs a later corpus.** The canonical
  snapshot still has the pre-fix post-07-28 quiet-source shape despite
  containing `quill-xnb`'s first admissions. Current-volume consistency
  and physical sizing are measured, but `quill-45m.27` must repeat them
  after full retained-source reconciliation. The ≤1M-row/day envelope
  remains the provisional bound until that evidence lands.
- **Backfill UX on slow disks.** The 20s stall machine is exactly where
  backfill is slowest. Mitigation: raw path + "building index" note
  keeps Models functional (degraded but honest) during the one-time
  backfill; progress events give an ETA; fresh installs no-op.

## Sequencing

Ordered work items with explicit edges — this becomes the bead DAG
(P0-P3 only; every item has verifiable acceptance criteria). Slices A
and B are independent and first; C/D/E are gated on re-measurement.
Convention: the Depends lists are the single source of truth for the
bead DAG; Blocks lists are informational mirrors.
Traceability: S1 → items 2,3,4,5,6,7,8,12; S2 → 2,9,10,13; S3 → 15,16;
S4 → 17,18; S5 → 19,20,21; Goal 6 → 6,11,12; Goal 7 → 1,14,22;
Q1 → 3,4,5,6; Q2 → 9,10,13; Q3 → 1; Q4 → 2 (framework item),7,8;
Q5 → 14; Q6 → 17,18; Q7 → 11,12,13,16.

1. **Benchmark harness and frozen-corpus protocol.** Freeze the 13.5 GB
   snapshot, build the spike-bin harness, record the BEFORE numbers for
   every touched query at 24h/30d/90d plus the 013 cached-only endpoints'
   cold cost. Acceptance: before-table committed; corpus location +
   pinned endpoints recorded. *Blocks: post-A/B re-measurement, rollup
   consistency (corpus leg), runtime parity, timing evidence.*
   Independent — first.
2. **Migration 37 additive rollup schema.** `model_usage_hourly`,
   `runtime_hourly`, `runtime_turn_state`, `rollup_meta`, indexes;
   version bump 36→37; migration comment + release-note text (one-way
   door). Acceptance: fresh and existing DBs migrate; older-build
   refusal path exercised; no backfill inside the migration.
   *Blocks: 3, 5, 9; also 4 (marker).* Independent of 1.
3. **Model rollup fold at ingest.** Same-transaction UPSERT fold on the
   observation write path; NULL-aware sums; `rollup_generation` bump.
   Acceptance: seeded ingest → rollup equals raw group-by; fold overhead
   within the ≤10% budget on burst-shaped batches. *Depends: 2. Blocks:
   4, 5, 6.*
4. **Rollup-aware cache probe marker.** Probe layer (storage.rs:203-318)
   includes `rollup_generation` for model/runtime commands. Acceptance:
   an UPSERT-only rollup write invalidates the 013 cache entry (the
   MAX-probe blindness case). *Depends: 2, 3. Blocks: 5, 10.*
5. **Hybrid read path for Models overview and history.** Closed hours
   from rollup + sources join, open hour raw, facets over the grain;
   `building_index` fallback while backfill incomplete; temp-table path
   scoped to fallback/open-hour. Acceptance: results equal raw
   computation on seeded DBs; facet inventory documented. *Depends: 3, 4.
   Blocks: 12, 14.*
6. **Source-keyed invalidation for deletion, re-ingest, and retention
   ordering.** Transactional rollup DELETE+refold on source delete
   (:2256) and re-ingest (:17888); suppression via read-time join;
   fold-then-prune precondition + `raw_pruned` marking in
   retention.rs:158-173 flow, mirrored for `runtime_hourly` on the
   session_events prune (bookmark clamps forward; pruned windows served
   from `raw_pruned=1` rows; no double-count against
   retention_daily_aggregates). Acceptance: each mutation leaves rollup
   == refold-from-raw; prune refuses without coverage, for both rollups.
   *Depends: 3; runtime leg depends: 9. Blocks: 12.*
7. **Chunked backfill framework with quiesce yield and resumable
   bookmark.** Shared machinery: bounded chunks (≤250ms target),
   per-chunk `wal_checkpoint(TRUNCATE)`, disk preflight re-checked
   per chunk (or per N chunks) with a typed error terminal event on
   mid-run ENOSPC, bookmark commit per chunk, lease yield
   (lib.rs:110-151), progress events. Acceptance: interrupt/resume test
   passes; lease acquired within one chunk bound.
   *Depends: 2. Blocks: 8, 10 (backfill leg), 11.*
8. **Model rollup backfill and rebuild command with Performance tab
   progress.** First-run backfill over existing 4.2M rows;
   `rebuild_model_rollup` (refuses under lease; preserves `raw_pruned`);
   PerformanceTab row + ModelsView "building index" note wired to the
   events. Acceptance: full backfill on the frozen corpus completes
   resumably; UI renders the enumerated states — in-progress with
   counts, refused-under-lease, error, completed; empty-DB backfill
   completes instantly and emits the terminal finished event; no rollup
   overhead regression on small DBs. *Depends: 5, 7. Blocks: 12, 14.*
9. **Time-invariant turn finalization and runtime rollup fold.** Closed
   turns finalized at ingest (gap = min(next−prev, 6h), pure f(data));
   fold into `runtime_hourly`; `runtime_turn_state` bookkeeping;
   re-ingest invalidation (:17888) for the runtime side. Acceptance:
   closed-turn values identical across re-queries; re-ingest leaves
   rollup == refold; finalization+fold overhead stays within the shared
   ≤10% added-p95 ingest budget, measured on burst-shaped
   session_events batches. *Depends: 2. Blocks: 10, 13.*
10. **Runtime stats hybrid read with open-turn tail and backfill.**
    `get_llm_runtime_stats` serves closed hours from rollup + open turns
    from the raw tail past the bookmark (INDEXED BY pin narrowed to the
    tail); runtime backfill over existing 2.9M session_events on the
    shared framework. Acceptance: ≤200ms @90d on frozen corpus (with 14);
    open-turn query reads only rows past `finalized_through_rowid`
    (row-count/EXPLAIN assertion). *Depends: 4 (rollup-aware probe —
    without it the MAX-only 013 probes serve stale runtime results after
    UPSERT-only folds), 7, 9. Blocks: 13, 14.*
11. **Backfill/quiesce concurrency test family.** Family 2 tests + lat.md
    spec sections, one `@lat:` ref each. Acceptance: tests green; specs
    linked; WAL-bound assertion in place. *Depends: 7, 8, 10 — the
    backfills under test must exist before the test can run.*
12. **Rollup consistency test family.** Family 1 tests (exactness,
    invalidation, fold-then-prune, crash consistency) + frozen-corpus
    equality leg + lat.md specs. Acceptance: closed buckets exact on
    corpus; pruned-window authority proven. *Depends: 5, 6, 8; corpus leg
    depends: 1.*
13. **Runtime parity tests with pinned now on frozen corpus.** Reference
    implementation of the redefined semantics; exact match at 24h/30d/90d
    with pinned `now`; lat.md spec. *Depends: 10; corpus depends: 1.*
14. **Post-A/B re-measurement on frozen corpus.** Re-run the harness for
    every touched query and the Usage/Charts/Context view fan-outs;
    record against the budget table; size or shrink C/D/E against the
    new baseline (Clarification Q5). Acceptance: committed interim
    table; explicit go/shrink decision per slice recorded. *Depends: 1,
    5, 8, 10. Blocks: 15, 17, 18, 19, 20, 21 — the C/D/E gate.*
15. **Reader connections for all view-serving reads and CPU off the
    lock.** Migrate the enumerated slice-C callers (API section) to
    short-lived read-only readers; move bucketing/downsampling outside
    the MutexGuard. Acceptance: no view query blocks another; no
    busy-timeout regressions under concurrent ingest. *Depends: 14.
    Blocks: 16.*
16. **5s-injection contention test family.** Family 3 test + lat.md
    spec: injected 5s query, fast class ≤100ms p95, zero lock errors.
    *Depends: 15.*
17. **Module-level frontend cache and ≥5s refresh coalesce.**
    useCachedInvoke module cache (survives ViewRegion unmount), coalesce
    constant ≥5000ms. Acceptance: view switch re-renders from cache with
    zero re-issued fan-out (background revalidate allowed); during
    active ingest a mounted view refreshes at ≥5s cadence (mock-verified
    + query-window log). *Depends: 14.*
18. **Range-honest queries: range×2 comparisons, Trends and skills
    scoping, duplicate breakdown removal.** useCodeInsights prior-period
    windows (:52-65), useWeeklyTrends 7d+prior-7d, UsageView skills
    range (:84), lazy second breakdown (:439-443). Acceptance:
    query-window log shows no window > R except the one range×2 prior
    period. *Depends: 14.*
19. **Drop full_input fetch from code stats paths.** storage.rs:16610/
    16733 select line counts when present, payload only for legacy rows;
    history loop restructured off the lock. Acceptance:
    `get_code_stats_history` ≤300ms @30d on corpus. *Depends: 14.*
20. **Time-bound session breakdown subqueries.** storage.rs:11222
    correlated subqueries time-bounded/restructured so LIMIT 200 prunes.
    Acceptance: ≤300ms @30d on corpus; identical results on seeded DBs.
    *Depends: 14.*
21. **Bounded ANALYZE with analysis_limit and plan audit.**
    `PRAGMA analysis_limit=1000` + ANALYZE from the maintenance path;
    before/after EXPLAIN QUERY PLAN audit of all touched queries on the
    corpus; `sqlite_stat1` exists after. Acceptance: no touched query
    regresses; audit committed. *Depends: 14, 19, 20 (last E item —
    highest blast radius).*
22. **Timing evidence pass and committed measurement doc.** Final
    before/after table for every touched query at 24h/30d/90d, view
    render budgets, warm-regression check —
    `specs/020-widget-query-perf/timing-measurement.md` (013 style).
    Gating (S5 AC). *Depends: 14, 15, 16 (the budget table includes the
    5s-injection ≤100ms p95 row item 16 produces), 17, 18, 19, 20, 21.*
23. **lat.md updates and lat check.** data-flow (rollup fold, hybrid
    read, fold-then-prune, time-invariant turn semantics), features,
    frontend (cache/coalesce/range rules), test-spec sections with
    `require-code-mention`. Acceptance: `lat check` green. *Depends:
    11, 12, 13, 16, 22 — the DAG's terminal items; every other item is
    upstream of one of these. Final.*

## Backlog Refinement

No P4 backlog inputs existed (spec "Backlog Inputs": no `source_backlog`,
no epic, `bd search` empty) — nothing to refine or close. The separate
**quill-xnb** P1 bug (post-2026-07-28 source-admission regression: new
rollouts on disk mostly never enumerated for ingest; true throughput
understated ~100×) is already filed and referenced by this plan only as
a sizing-validation dependency (Data Model, Risks); disposition:
**not-in-scope** — this feature does not fix it. The first follow-up is
resolved: Sharaf Nassar, project maintainer, owns communication about the
2.5 GB `usage.db.pre-model-wipe.bak`. The release notes tell users that the
backup is optional and user-controlled, and to delete it only after confirming
the current database works and the backup is no longer needed. Quill will not
delete it automatically (spec Non-Goal). One follow-up bead remains to re-run
the rollup sizing/consistency validation after quill-xnb is fixed — the
revalidation promised in Data Model and Risks needs an owner.

## Target Epic

None exists. A new epic for widget-query-perf will be created at
create-beads time; the Sequencing items above become its P0-P3 beads
with the stated depends/blocks edges (P4 forbidden — every item has
concrete, verifiable acceptance criteria).

## Alignment fixes applied

- Item 3 no longer Blocks 7 — the backfill framework is independent of
  the model fold; slices A and B stay independent after migration 37.
  (B/must)
- Item 10 now Depends on 4 — the rollup-aware `rollup_generation` probe
  is required so the MAX-only 013 cache probes never serve stale
  runtime results after UPSERT-only folds. (A/must)
- Item 11's "exercises 8, 10" became real Depends edges (7, 8, 10) —
  the concurrency tests cannot run before their backfill subjects
  exist. (B/must)
- Item 9 gained an acceptance criterion holding finalization+fold
  within the shared ≤10% added-p95 ingest budget on burst-shaped
  session_events batches. (A/must)
- `runtime_hourly` now mirrors the model rollup's retention story:
  `raw_pruned` flag + fold-then-prune precondition (Data Model),
  bookmark clamp-forward on pruned rowids (`runtime_turn_state`),
  item 6 and Testing Family 1 extended to cover it, and an explicit
  reconciliation — `runtime_hourly` wins for runtime stats;
  `retention_daily_aggregates` (which retention DOES write for
  session_events) remain for token/code totals only. (A/must)
- Slice-C scoping no longer contradicts S3: ALL view-serving reads move
  off the writer connection; re-measurement decides migration priority
  and effort sizing, not whether a path migrates. (A/should)
- The rebuild command takes a `target` (`model` | `runtime`) so the
  rebuild-from-raw observability path covers `runtime_hourly`,
  preserving `raw_pruned=1` coverage for either target. (B/should)
- Data Model states the existing 7 indexes / 3.6 GB on
  `model_usage_observations` are kept as-is; index cleanup is an
  explicit follow-up outside this feature (per Non-Goal). (A/should)
- Item 22 now Depends on 16 — its budget table includes the
  5s-injection ≤100ms p95 row item 16 produces. (B/should)
- Item 23's "all implementation items" replaced with the enumerated
  terminal items 11, 12, 13, 16, 22. (B/should)
- Item 8 acceptance enumerates the UI states (in-progress with counts,
  refused-under-lease, error, completed) and adds the fresh-install
  no-op: empty-DB backfill completes instantly, emits the terminal
  finished event, no small-DB overhead regression. (B/should)
- Item 7's disk preflight is re-checked per chunk (or per N chunks)
  with a typed error terminal event on mid-run ENOSPC. (B/should)
- Item 10's vague "tail scan bounded" replaced with "open-turn query
  reads only rows past `finalized_through_rowid`" plus a
  row-count/EXPLAIN assertion. (B/should)
- Backlog Refinement files a second follow-up bead: re-run rollup
  sizing/consistency validation after quill-xnb is fixed, giving that
  promise an owner. (B/should)
- Sequencing preamble records the convention that Depends lists are the
  single source of truth for the bead DAG; Blocks lists are
  informational mirrors (resolving the item 5/8 and 9/13 asymmetries).
  (B/should)
