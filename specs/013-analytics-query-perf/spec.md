# Spec: analytics-query-perf

## Problem Statement

Switching timeframes (1h/24h/7d/30d/all) in the analytics dashboard takes
noticeably long to load new data. A two-layer audit (frontend fetch-path trace
+ backend SQL audit with `EXPLAIN QUERY PLAN` against the real 7.45 GB
`usage.db`) found the slowness is not one bug but five compounding causes:

1. **Physically bloated database.** `usage.db` is 7.45 GB, `auto_vacuum=0`,
   never vacuumed. ~0.87 GB is a dead `tool_actions_legacy_v30` table left
   behind by a migration and never dropped. Every scan traverses a huge file
   with poor page locality, amplifying all other costs.
2. **Uncached full-range backend aggregations (Models tab worst).**
   `get_model_usage_overview` (`storage.rs:6422`) re-materializes a temp table
   of up to ~956k `model_usage_observations` rows and builds two temp indexes
   on every call; `get_model_analytics` (`storage.rs:6097`) and
   `get_model_history` (`storage.rs:7317`) re-scan/`GROUP BY` the same range.
   No analytics command has result caching (only the live usage-bucket cache
   at `storage.rs:1539`).
3. **Frontend hooks with no cache, no debounce, no cancellation.** All
   Now/Trends/Charts/Context hooks are hand-rolled `useState` +
   `useEffect(invoke)` keyed on `range`. Switching back to an already-viewed
   range refetches everything; rapid switching stacks requests and stale
   responses can overwrite newer ones. One Now-tab switch fires ~13 concurrent
   IPC calls, including duplicate `get_llm_runtime_stats` and
   `get_snapshot_count` calls and a range-independent `get_token_hostnames`
   refetch. The Models-tab hooks (`useModelAnalytics.ts:205, 255-273`) already
   implement the correct pattern: per-scope cache, 200 ms debounce, in-flight
   dedupe.
4. **Backend N+1 / multi-pass patterns.** `get_all_bucket_stats`
   (`storage.rs:9053`) issues 3 queries per bucket (stats with a correlated
   subquery + 2× `calc_trend`); `get_context_savings_analytics`
   (`storage.rs:9558`) runs ~8 sequential aggregate passes per call.
5. **Latent O(total-rows) scans.** `get_session_stats`,
   `get_session_breakdown`, `get_host_breakdown`, `get_project_breakdown`
   full-scan `token_snapshots` and filter the timestamp per row instead of
   pruning via `idx_token_snap_ts` (EQP-confirmed); `get_session_stats` wraps
   the column in `strftime(...)`. Cheap at today's 2,315 rows, but cost is
   O(total history) and a narrower timeframe gives zero benefit.

Affected users: everyone using the analytics view of the Quill desktop app;
severity grows with usage history (the DB only gets bigger).

## Goals

- Timeframe switches on every tab feel fast: switching to a **previously
  viewed** range renders from cache near-instantly; a **cold** switch is
  bounded by one non-duplicated fetch set.
- Models-tab wide-range (30d/all) switches no longer re-materialize temp
  tables per call; repeated calls with the same `(range, provider)` are served
  from a backend cache invalidated on ingest.
- Rapid consecutive switches never race: the last selected range always wins
  and in-flight work for abandoned ranges is ignored or cancelled.
- Redundant IPC eliminated: no duplicate same-args commands per switch;
  range-independent data is not refetched on range change.
- The database sheds its dead weight: `tool_actions_legacy_v30` dropped and
  space reclaimed (one-time VACUUM and/or incremental autovacuum going
  forward).
- Token-breakdown queries prune by the timestamp index so their cost scales
  with the selected timeframe, not total history.
- Guidance targets (per Clarifications Q4 — engineering guidance, not hard
  acceptance): wide-range Models-tab switch p50 under ~300 ms warm / ~1.5 s
  cold on the current 7.45 GB dataset; repeat-switch on other tabs rendered
  from cache with a background revalidate.
- Lightweight per-command timing logs ship with the feature so before/after
  latency is measurable on the real dataset and in the field.

## Non-Goals

- No redesign of the analytics UI, tabs, chart types, or the timeframe
  selector UX.
- No change to what data is collected or how ingest/hook pipelines write to
  the DB (only how analytics reads it, plus dropping a dead table).
- No general-purpose data layer migration (e.g. adopting react-query across
  the whole app) — only the analytics hooks in scope, reusing the existing
  in-repo pattern.
- No pruning/retention policy for large live tables (`tool_actions`,
  `session_events`) — that is a separate data-lifecycle feature.
- No pre-computed rollup/materialized-view pipeline for model analytics
  (evaluate later if caching proves insufficient).
- No changes to non-analytics IPC commands.
- **Deferred by Clarifications Q6:** S6 (token-breakdown query rewrites —
  cheap at today's row counts, and the review found the draft partly
  mischaracterized them) and the S7 query rewrites (cache those endpoints
  first; rewrite only if cold cost still misses the guidance targets).
  A follow-up bead will be filed for retention/pruning of `tool_actions`
  (1.69 GB) + `session_events` (0.77 GB), which dominate remaining file
  size.

## User Stories

### S1: Fast repeat timeframe switching
As an analytics user, I want switching back to a timeframe I already viewed to
render instantly, so that comparing ranges (24h ↔ 7d ↔ 30d) is fluid.

**Acceptance criteria:**
- Switching to a range already fetched this session renders cached data
  immediately with no loading state (stale-while-revalidate per
  Clarifications Q7: a background refetch always runs and fresh results are
  swapped in when they arrive).
- Staleness is bounded: cached entries carry a TTL (~30-60 s per
  Clarifications Q3); entries past TTL still render instantly but always
  revalidate.
- Cache is invalidated when ingest events (`tokens-updated`,
  `sessions-index-updated`, `transcript-analytics-updated`) arrive, at
  per-table granularity (an event only invalidates caches reading the
  affected tables).

### S2: Race-free rapid switching
As an analytics user, I want quickly clicking through ranges to settle on the
last one I picked, so that I never see data from a range I abandoned.

**Acceptance criteria:**
- Firing 4 rapid switches results in at most one settled fetch set (debounced),
  and the rendered data always corresponds to the final selected range.
- Stale responses (from an earlier range) can never overwrite newer results —
  verified by a request-id/abort guard in every analytics hook.

### S3: Fast Models tab on wide ranges
As an analytics user, I want the Models tab to load 30d/all ranges quickly,
so that long-horizon model comparisons are usable.

**Acceptance criteria:**
- `get_model_usage_overview` no longer pays temp-table + temp-index
  materialization on repeat calls: an in-process backend cache keyed
  `(command, range, provider, time-bucket)` (Clarifications Q2) serves
  unchanged data.
- Cache reads verify a cheap per-table data version (covering out-of-process
  hook writes and the background model-backfill worker) and honor a TTL
  bounding sliding-window drift; new model-observation ingest invalidates
  only model-sourced entries.
- Cold-call cost is at worst one full indexed range aggregation, not
  materialize + index + multi-pass (benchmark before removing the temp table
  entirely — see Open Questions #7).

### S4: No redundant fetches per switch
As a developer, I want each timeframe switch to issue a minimal, deduplicated
set of IPC calls, so that backend load and IPC serialization stay bounded.

**Acceptance criteria:**
- One Now-tab switch issues no duplicate command+args pairs (today:
  `get_llm_runtime_stats` ×2, `get_snapshot_count` ×2).
- `get_token_hostnames` (range-independent) is not refetched on range change.
- The `useCodeInsights` comparison-range fetches reuse already-fetched
  current-window series where possible instead of issuing overlapping
  history queries.

### S5: Database reclaims dead space
As a user with a long history, I want the app's database to stop carrying a
dead 0.87 GB table, so that disk usage drops and every query touches fewer
pages.

**Acceptance criteria (per Clarifications Q1 — two separate deliverables):**
- Migration v34 drops `tool_actions_legacy_v30` only (fast; no VACUUM inside
  the migration). Idempotent on DBs where the table never existed or was
  already dropped. Bumps `MAX_SUPPORTED_SCHEMA_VERSION`; the one-way door is
  documented (Clarifications Q5).
- A separate "Compact database" operation (user-triggered or idle-triggered)
  runs VACUUM with: a preflight free-disk check (~2× file size), ingest
  quiesced for the duration, progress UI, and skip-and-report on any failure
  (insufficient disk, interruption) — never silent data risk.
- After a successful compaction, file size shrinks measurably (the dead
  table's ~0.87 GB plus freelist).

### S6 (DEFERRED — Clarifications Q6): Timeframe-proportional breakdown queries
As a developer, I want the token-breakdown queries to prune by the timestamp
index, so that their cost scales with the selected timeframe instead of total
history.

**Acceptance criteria:**
- `get_session_stats`, `get_session_breakdown`, `get_host_breakdown`,
  `get_project_breakdown` show `SEARCH ... (timestamp>?)` (or equivalent
  index-pruned plans) in `EXPLAIN QUERY PLAN`, not full-table/index `SCAN`
  with per-row timestamp filtering.
- No `strftime()`/function wrapping on the filtered timestamp column.
- Per-output-row correlated subqueries in `get_session_breakdown` are either
  retained deliberately (documented as bounded by row limit) or folded into
  the main query.

### S7 (PARTIALLY DEFERRED — Clarifications Q6, cache-first): Reduced N+1 and multi-pass backend work
As a developer, I want `get_all_bucket_stats` and
`get_context_savings_analytics` to issue a bounded number of queries, so that
Now/Trends/Context tab switches don't fan out linearly.

**Acceptance criteria:**
- `get_all_bucket_stats` computes all buckets' stats + trends in a constant
  number of grouped queries (no per-bucket loop).
- `get_context_savings_analytics` collapses its ~8 sequential passes where
  practical (single-pass aggregates or combined breakdown query), or the
  remaining passes are justified and cached.

## Constraints

- **Backend:** Rust, `src-tauri/src/storage.rs` (SQLite, WAL mode, 5 s busy
  timeout, schema migration v33 current). IPC handlers in
  `src-tauri/src/lib.rs` run via `spawn_blocking`. New work lands as
  migration v34+.
- **Frontend:** React + TypeScript, analytics components in
  `src/components/analytics/`, hooks in `src/hooks/`. Reuse the in-repo
  `useModelAnalytics` cache/debounce/dedupe pattern rather than introducing a
  new dependency (react-query et al. are out of scope per Non-Goals).
- **Fix directions ranked by leverage (from the audit):**
  1. Drop `tool_actions_legacy_v30`, then VACUUM / incremental autovacuum.
  2. Cache Models-tab aggregates keyed on `(range, provider)`, invalidated by
     ingest events.
  3. Port the `useModelAnalytics` pattern (per-scope cache + 200 ms debounce +
     in-flight dedupe) to Now/Trends/Charts/Context hooks.
  4. Dedupe duplicate IPC calls; hoist `get_token_hostnames` out of the
     range-keyed fetch.
  5. Rewrite token-breakdown queries to prune via `idx_token_snap_ts`; drop
     `strftime`-on-column.
- **Data safety:** the DB is the user's only usage history — migrations must
  be non-destructive except for the explicitly dead legacy table. VACUUM
  requires up to 2× free disk space transiently.
- **Concurrency:** writers (ingest hooks) run concurrently with analytics
  reads under WAL; a long VACUUM blocks writers — timing/strategy matters.
- **Evidence base:** `EXPLAIN QUERY PLAN` runs were read-only
  (`immutable=1`) against `~/.local/share/com.quilltoolkit.app/usage.db`;
  regressions should be re-checked the same way.

## Open Questions

1. **VACUUM strategy.** One-time VACUUM of a 7.45 GB file can take minutes
   and needs ~2× disk headroom. Run at startup with progress UI? Background
   with a "compacting" indicator? Or switch to `auto_vacuum=INCREMENTAL`
   (requires VACUUM once anyway to take effect) and amortize?
2. **Backend cache shape.** In-process memory keyed `(command, range,
   provider)` with event-driven invalidation, or a small `analytics_cache`
   table with data-version stamps? Memory is simpler; table survives restarts.
3. **Cache invalidation granularity.** Invalidate all analytics caches on any
   ingest event, or track per-table data versions so e.g. context-savings
   ingest doesn't blow the model cache?
4. **Frontend cache extraction.** Extract the `useModelAnalytics`
   cache/debounce machinery into a shared `useCachedInvoke`-style utility, or
   copy the pattern per hook? Shared utility touches more files but prevents
   drift.
5. **Numeric performance targets.** Are the proposed targets (warm switch
   < 300 ms, cold wide-range Models < 1.5 s) the right bar, and on what
   hardware baseline are they measured?
6. **`get_session_breakdown` correlated subqueries.** Fold the four per-row
   subqueries (turn_count, last_active, project, subagent_count) into joined
   CTEs, or keep them (bounded by output-row limit) and only fix the outer
   scan?
7. **Temp-table replacement for `get_model_usage_overview`.** Replace with
   CTE-based single-pass aggregation, or keep materialization but cache the
   result? (Materialization may still win when 6 subqueries genuinely reuse
   the scoped set — needs a benchmark.)
8. **`SnapshotGate` duplicate.** Should the gate's fixed-24h
   `get_snapshot_count` share state with the tab-level hook via context, or is
   lifting the hook to a common parent enough?
9. **Does the Context tab need the full ~8-pass analytics payload for
   `limit:1` calls from the Now tab**, or should a lighter summary-only
   command variant exist?

## Clarifications

Human answers to the critical questions (all recommended options chosen).
The body sections above/below have been updated to reflect them.

**Q1: VACUUM strategy?**
A: Dedicated maintenance path. Migration v34 drops
`tool_actions_legacy_v30` (fast, no VACUUM inside the migration). VACUUM
runs separately as a user-triggered or idle-triggered "Compact database"
operation with: preflight free-disk check (~2× file size), quiesced ingest
during the run, progress UI, and a skip-and-report branch on any failure.

**Q2: Backend cache shape?**
A: In-process memory cache keyed `(command, range, provider, time-bucket)`
with a short TTL bounding sliding-window drift and missed events, plus a
cheap DB data-version check (e.g. max rowid / updated-at per source table)
on read so out-of-process writes (hooks, widget, backfill worker)
invalidate correctly.

**Q3: Invalidation granularity and staleness bound?**
A: Per-table data versions — e.g. context-savings ingest does not
invalidate the model cache — with a TTL cap (~30-60 s) as the hard
staleness bound.

**Q4: Performance bar and measurement?**
A: The p50 targets are engineering guidance, not hard acceptance criteria.
Ship lightweight per-command timing logs so before/after is measured on the
real 7.45 GB DB. No CI perf harness this round.

**Q5: Migration one-way door?**
A: Accept and document. Single-user local app; the legacy table is provably
dead. v34 bumps `MAX_SUPPORTED_SCHEMA_VERSION`; no backup gate.

**Q6: Scope cut?**
A: MVP = S1–S4 plus the S5 table-drop (per Q1). S6 (latent scans) and the
S7 query rewrites are deferred — cache the S7 endpoints first and rewrite
only if cold cost still misses the guidance targets. File a follow-up bead
for retention/pruning of `tool_actions` + `session_events` (2.46 GB).

**Q7: Now-tab liveness vs cache?**
A: Stale-while-revalidate everywhere: render cached data instantly, always
refetch in the background, swap in fresh results; live ingest events still
force a refresh.

## Spec Review

Six parallel review passes (requirements, gaps, ambiguity, feasibility,
scope, stakeholders) were run against this draft. Cross-dimension hits are
merged below; feasibility findings that correct the draft's claims are
called out explicitly.

### Critical Questions (answer before planning)

1. **VACUUM strategy — the hardest problem, and worse than the draft
   states.** The app holds a single `Mutex<Connection>` (`storage.rs:2780`),
   so a minutes-long VACUUM blocks *every* read and write IPC, not just
   writers; the separate model-reader connection hits `SQLITE_BUSY` past its
   5 s timeout; out-of-process ingest hooks fail their writes (dropping
   usage data — the user's only history); and `DROP TABLE
   tool_actions_legacy_v30` alone reclaims **zero** OS bytes in WAL mode —
   only a full VACUUM shrinks the file, and `auto_vacuum=INCREMENTAL`
   itself requires one full VACUUM to take effect. Needs: a dedicated
   maintenance connection + quiescent window, a preflight free-disk check
   (~2× file, ≈15 GB) with a skip-and-report branch, defined
   abort/disk-full recovery, and a decision on whether non-analytics users
   pay this cost. — flagged by: all six dimensions.
2. **Cache correctness across processes and sliding windows.** Ingest hooks
   (and possibly the widget) write to SQLite from *separate processes*; an
   in-process memory cache invalidated by in-app events can go permanently
   stale. Separately, `(range, provider)` is an incorrect cache key:
   `range_end = Utc::now()` (`storage.rs:6427-6432`) makes every range a
   sliding window, so a cache invalidated only on ingest serves a
   stale-boundary window as wall-clock advances — a time-bucket/TTL
   dimension is required. Any data-version scheme must also cover the
   background model-backfill writer, not just foreground ingest. — flagged
   by: stakeholders, feasibility, ambiguity, scope.
3. **Invalidation granularity and the definition of "stale".** Invalidate
   all analytics caches on any ingest event (simple, but under active
   ingest collapses the cache to a passthrough — exactly when users watch
   analytics) vs. per-table data versions? And what bounds staleness when
   an event is missed (TTL / max-staleness cap / stale indicator)? "Stale"
   is currently undefined, so S1's background-refresh criterion is
   untestable. — flagged by: ambiguity, requirements, gaps, scope.
4. **Performance targets have no baseline and no harness.** No bench
   tooling exists under `src-tauri/`; the p50 targets (< 300 ms warm /
   < 1.5 s cold) name no hardware, dataset fixture, or measurement method —
   and they gate whether the S7 query rewrites are needed at all
   (cache-first may clear the bar). Decide the bar, the reference dataset,
   and whether a minimal benchmark/timing-log harness is in scope. —
   flagged by: requirements, ambiguity, gaps, scope.
5. **Migration v34 is a one-way door.** It must bump
   `MAX_SUPPORTED_SCHEMA_VERSION` (`storage.rs:58`); once a v34 build opens
   the DB, older builds hard-refuse it (`SCHEMA_TOO_NEW`) — combined with
   an irreversible DROP + VACUUM there is no downgrade story. Accept and
   document, or provide a mitigation? — flagged by: gaps.
6. **Scope cut: is the MVP S1–S4, with S5 split out and S6/S7 deferred?**
   Scope review recommends: ship the cache/debounce/dedupe family (S1–S4)
   as the user-visible MVP; split S5 (DB hygiene) into its own deliverable
   (distinct risk profile, blocked on Q1); defer S6 (explicitly "cheap
   today"); and cache S7's endpoints first, rewriting the multi-pass
   queries only if cold cost still misses the target. — flagged by: scope,
   feasibility.
7. **Now-tab liveness vs. cache-first.** The Now tab is effectively a live
   dashboard; rendering cached data "with no loading state and no IPC"
   conflicts with a live-monitoring user's freshness expectation. Should
   the live path get an explicit freshness policy distinct from historical
   ranges? — flagged by: stakeholders, ambiguity.

### Non-Blocking Observations

- **S6 is partly mischaracterized** (feasibility): `get_session_stats`
  already filters sargably (`WHERE timestamp >= ?1`, `storage.rs:10591`);
  its `strftime` sits on `MAX/MIN(timestamp)` in the projection, not the
  filter; and for the "all" range a full scan is the *correct* plan. Recast
  S6 as a GROUP-BY-shape/index-choice question, and drop the blanket
  "SEARCH not SCAN" acceptance criterion for wide ranges.
- **`get_all_bucket_stats` N+1 is bounded by live-bucket count (single
  digits)** — feasible to collapse but the lowest-leverage target; don't
  let it compete with Q1–Q3 work.
- **Verify uniform RFC3339 offsets** in stored timestamps before relying on
  lexicographic `timestamp > ?` pruning (one-time check on the real DB).
- **"Where possible / where practical" hedges** in S4/S7 make those
  criteria non-binding; set a concrete minimum (e.g. target query count)
  during planning.
- **No field observability**: no shipped timing telemetry for analytics
  commands, so post-release "still slow" reports can't be triaged; consider
  lightweight per-command timing logs.
- **Failure states are unspecified** across stories: backend query error,
  cache-miss fallback, migration failure/interruption recovery.
- **Empty/fresh-install and zero-result states** are unaddressed (empty
  timeframe caching, no-data render).
- **The day-after ask is predictable**: `tool_actions` (1.69 GB) +
  `session_events` (0.77 GB) dwarf the dead table being dropped; file the
  retention/pruning follow-up now so S5's "disk usage drops" isn't
  oversold.
- **Precedent risk**: putting a minutes-long VACUUM inside a schema
  migration teaches future migration authors that blocking compaction
  belongs there; take an explicit stance.
