# Spec: widget-query-perf

## Problem Statement

Widget views (Usage, Trends, Charts, Models, Context) load slowly and get
disproportionately slower on larger timeframes. Measured 2026-08-02 on the
live 13.5 GB `usage.db` (read-only, real data):

- **Models is the disaster.** `get_model_usage_overview`'s scoped SELECT
  (`storage.rs:7201`) materializes ~4.03M `model_usage_observations` rows
  into a temp table at 30d: **~20s**. The model history aggregate
  (`storage.rs:~8095`) takes **7–14s**. Both are "index seek" on paper but
  the range index is non-covering, so 4M matched rows mean 4M random heap
  fetches into a 2.5 GB table plus 4M join probes into
  `model_observation_sources` that filter almost nothing (0 suppressed).
- **`get_llm_runtime_stats` scales linearly with raw events.**
  `storage.rs:17370` walks every `session_events` row in the window with a
  temp b-tree sort and per-row RFC3339 parsing in Rust: 41ms @24h → ~1s
  @30d → ~2s @90d. It feeds Usage's runtime readout, Trends (fixed 30d+7d),
  and code insights.
- **One `Mutex<Connection>` serializes everything.** All view queries except
  model analytics share the connection at `storage.rs:3165` with ingest
  writes. WAL does not help within one connection. A 20s Models compute or
  1s runtime walk head-of-line-blocks every other band — fast (≤5ms)
  snapshot queries queue behind slow ones. The MutexGuard is also held
  through post-query CPU work (bucketing, downsampling).
- **Refresh amplification.** `tokens-updated` fires per ingested turn
  (server.rs:308), 1s debounce → Usage's ~13-query fan-out re-runs every few
  seconds during active LLM use. The `useCachedInvoke` cache is a `useRef`
  per component instance (`useCachedInvoke.ts:75`) and `ViewRegion` unmounts
  the inactive view (`ViewRegion.tsx:111`), so every view switch is a
  cold-query storm. The context-savings cache is invalidated by any new
  event row, so it never holds during active MCP use.
- **Hidden range inflation.** `useCodeInsights` queries at the next-larger
  comparison range (`useCodeInsights.ts:52-65`: 7d selection → three 30d
  scans); Trends always scans 30d; the widget skills breakdown is hardcoded
  all-time (`UsageView.tsx:84` → no timestamp predicate); UsageView mounts
  an unconditional second `useBreakdownData("projects")`
  (`UsageView.tsx:439-443`).
- **Per-row payload waste.** `get_code_stats` / `get_code_stats_history`
  (`storage.rs:16616`, `16753`) fetch the full `full_input` TEXT payload
  (~4 KB/row, 488k `tool_actions` rows) for every code-change row even when
  migration-33 stored line counts make it unnecessary; history then runs an
  O(buckets × changes) nested loop while holding the connection lock.
- **`get_session_breakdown`** (`storage.rs:11258-11327`) evaluates
  un-time-bounded correlated subqueries (including a four-table UNION count)
  for every session in range before its `LIMIT 200` can apply.
- **Planner flies blind.** `ANALYZE` has never run (no `sqlite_stat1`);
  the code already pins one query with `INDEXED BY` to dodge a misplan.

Data shape: `model_usage_observations` 4.2M rows (96% ingested in the last
30 days — so 30d ≈ 90d ≈ worst case), `session_events` 2.9M,
`tool_actions` 488k at ~4 KB/row, `token_snapshots` only ~3k (healthy —
its `token_hourly`/`usage_hourly` rollup keeps token queries at single-digit
ms and is the pattern to replicate).

Prior art: `specs/013-analytics-query-perf/` fixed the same symptom at
7.45 GB with caching (45s TTL + table high-water probes), `useCachedInvoke`,
migration 34 (dead-table drop), and manual compaction. Those mitigations
help the warm path only; the data volume has since doubled and the cold
compute and single-connection serialization remain. This feature does the
structural work 013 deferred.

Why now: the ingest explosion (96% of model observations in 30 days) means
the product's default views are already at worst case, and it degrades
further with every active day.

## Goals

Success is measured on the live 13.5 GB database (or the frozen retention
corpus if the live DB is unavailable), per constitution principle 10:

1. **Models view cold load ≤500ms at 30d and 90d** (from ~20s). Target
   mechanism: pre-aggregated rollup for model observations mirroring
   `token_hourly`, so overview/history read O(buckets), not O(rows).
2. **`get_llm_runtime_stats` ≤200ms at 90d** (from ~2s). Mechanism open:
   runtime-stat rollup, incremental aggregation, or SQL-side turn-gap
   computation — decided at plan time.
3. **No view query blocks another.** View reads run on read-only
   connection(s) separate from the ingest-write connection (extend the
   existing `open_model_analytics_reader` pattern, `storage.rs:6216`).
   A deliberately slow query in one band must not delay other bands'
   queries by more than lock-free scheduling noise.
4. **Refresh work proportional to what the user selected.** No hidden
   range inflation: the only permitted over-range query is the
   comparison-delta prior period (range×2, per clarification Q6d);
   Trends and skills breakdown query the visible range. View switches
   within a session serve cached data instantly (module-level or backend
   cache, not per-instance).
5. **`get_code_stats*` stop fetching `full_input`** for rows with stored
   line counts; `get_session_breakdown` subqueries are time-bounded or
   restructured so LIMIT prunes work.
6. **Rollups tell the truth** (principle 1): aggregates must be exactly
   consistent with raw rows at rollup boundaries, gaps stay explicit, and
   retention/pruning (feature 014) semantics are preserved for rolled-up
   windows.
7. **Reproducible evidence**: timing measurements before/after in the house
   spike-bin style (`eqp_index_drop_spike.rs`, `retention_spike.rs`,
   `specs/013-*/timing-measurement.md`), committed with the spec artifacts.

## Non-Goals

- Redesigning widget view UI/UX or changing what the views display
  (018-widget-ui-redesign owns visuals; DESIGN.md migration exceptions
  stand).
- Retention/pruning policy changes beyond keeping feature-014 semantics
  correct over rollups.
- Off-device telemetry or remote analytics of any kind (principle 11).
- Fixing the Manage window / settings analytics surfaces except where they
  share the exact queries being fixed.
- Automatic scheduled VACUUM/compaction (013 decided manual-only; not
  revisited here).
- Deleting user data: the 2.5 GB `usage.db.pre-model-wipe.bak` cleanup is a
  user-communication item, not an automated deletion.
- General index redesign of the 6 GB index overhead beyond what the chosen
  rollup/covering-index approach requires.

## Backlog Inputs

None. No `source_backlog` or `epic` was supplied; `bd search` for
performance/slow-query/rollup terms found no open issues; no epic closure
exists yet, so there are no P4 sources to refine.

## Target Epic

None exists. This run will create the feature epic at `create-beads` time.

## User Stories

### S1 — Models view at any timeframe

As a Quill user with months of heavy usage, I want the Models view to load
in well under a second at 30d/90d, so that checking "which models did the
work" is instant instead of a 20-second stall.

**Acceptance criteria:**
- Cold (first in-process call, app caches bypassed)
  `get_model_usage_overview` and model history at 30d and 90d each
  complete in ≤500ms per query on the frozen 13.5 GB benchmark corpus
  (measured, logged).
- Closed-bucket results are exactly equal (integer sums; distinct counts
  via the source/session-keyed grain) to the raw-row computation on the
  frozen corpus; the open-hour tail is served from raw rows so no
  approximation exists anywhere (rollup consistency test, authorized).
- Backfill of the rollup over existing 4.2M rows completes without blocking
  ingest or the UI (bounded background work, principle 3), is resumable if
  interrupted (principle 4), and reports progress.

### S2 — Runtime stats without raw-event walks

As a Quill user, I want the runtime/velocity numbers in Usage and Trends to
appear without multi-second scans, so that the default view is responsive
during active sessions.

**Acceptance criteria:**
- `get_llm_runtime_stats` ≤200ms at 90d on the frozen benchmark corpus.
- Turn-gap semantics are redefined time-invariant (closed turns finalized
  at ingest; only open turns depend on `now`); the aggregated path
  matches the redefined reference computation exactly on the frozen
  corpus with pinned `now`, and closed-turn values never change on
  re-query. Re-ingest (per-source DELETE+reinsert) invalidates exactly
  that source's aggregates.

### S3 — Independent view reads

As a Quill user, I want switching bands or views to never wait behind an
unrelated slow query or an ingest write, so that fast queries feel fast.

**Acceptance criteria:**
- View-serving read queries execute on read-only connection(s) distinct
  from the write connection; ingest continues to serialize writes
  (principle 4).
- With an artificial 5s query injected on one path, other view queries
  complete within their budgets (fast-class queries ≤100ms p95).
- No `database is locked` / busy-timeout regressions under concurrent
  ingest + view refresh (existing busy timeout honored).

### S4 — Honest refresh work

As a Quill user on the 7d range, I want the app to query 7d of data, so
that selecting a smaller window actually costs less.

**Acceptance criteria:**
- Selecting range R issues no query whose WHERE window exceeds R, except
  the comparison-delta prior period (window exactly 2×R ending at R's
  start) — the single enumerated, justified over-range case.
- Widget skills breakdown is range-scoped (all-time was confirmed a bug
  at the clarify gate).
- Switching between views and back re-renders from cache without re-issuing
  the full query fan-out; a background revalidate is allowed.
- During active ingest, a mounted view's query fan-out is coalesced to a
  cadence of ≥5s (normative; supersedes 013 clarification Q7).

### S5 — Query-level cleanups hold up

As a maintainer, I want the known pathological queries fixed and guarded,
so that the next data doubling doesn't resurrect the symptom.

**Acceptance criteria:**
- `get_code_stats`/`get_code_stats_history` no longer select `full_input`
  for rows with stored line counts; `get_code_stats_history` ≤300ms @30d
  on the frozen corpus.
- `get_session_breakdown` no longer runs unbounded correlated subqueries
  per session; ≤300ms @30d on the frozen corpus.
- `ANALYZE`/`PRAGMA optimize` runs at a defined, bounded point (startup
  budgeted, post-migration, or manual maintenance — decided in plan);
  `sqlite_stat1` exists afterward.
- Timing evidence for every touched query at 24h/30d/90d, before/after,
  committed under `specs/020-widget-query-perf/`.

## Constraints

- **Stack/boundaries (principle 2):** Rust/Tauri storage + IPC layers,
  React strict-TS frontend. SQLite via rusqlite; no new database engine.
- **Single-writer reality:** ingest writes arrive from multiple sources
  (hooks, MCP server, pollers) through the storage layer; writes must stay
  serialized and transactional (principle 4). Read-only connections must
  open with `SQLITE_OPEN_READ_ONLY | NO_MUTEX` per the existing
  `open_model_analytics_reader` pattern.
- **Migration one-way door:** schema changes bump
  `MAX_SUPPORTED_SCHEMA_VERSION` (36 → N); older builds refuse newer DBs.
  Rollup tables must be additive; raw tables remain the source of truth
  (principle 1).
- **Retention interplay (feature 014):** pruning deletes source-owned rows
  from `tool_actions`, `session_events`, **and `model_usage_observations`**
  (`retention.rs:158-173`), and the delete engine deliberately writes NO
  `retention_daily_aggregates` row for model observations
  (`retention_engine.rs:1200-1202`) — after a prune, a model rollup would
  be the only surviving record for the pruned window. Any new rollup must
  define behavior when its raw rows are pruned, must not double-count
  against retention aggregates, and must reconcile "raw rows remain the
  source of truth" with this reality.
- **Existing cache layer:** 45s TTL + table high-water probes
  (`storage.rs:232-318`) from 013. New work must compose with it, not
  bypass it.
- **`block_in_place` (lib.rs:1658):** commands park tokio workers, not the
  GTK thread; changing the threading model is in scope only as far as
  moving reads off the shared mutex requires.
- **Live DB facts to design against:** 13.5 GB file, 205 MB
  un-checkpointed WAL, freelist ~2% (fragmentation is not the problem),
  7 indexes / 3.6 GB on `model_usage_observations` (the range index is
  non-covering), covering `idx_se_timestamp_chain` already exists on
  `session_events`.
- **Quality gates (principles 6, 8):** zero-warning fmt/lint/typecheck/
  build/tests; `lat.md` updated (backend DB schema, IPC commands, frontend
  hooks sections) and `lat check` green before completion.
- **Testing (principle 7):** authorized at the clarify gate for three
  families — rollup consistency, backfill/quiesce concurrency, and the
  5s-injection contention test — each linked one-to-one with lat.md specs.

## Open Questions

1. **Rollup granularity and dimensionality for model observations.**
   Hourly like `token_hourly`? Keyed by (hour, provider, derived_model_id)?
   The overview also needs session/source facets — which facets must the
   rollup preserve, and which can fall back to raw-row queries at small
   ranges?
2. **Runtime-stats approach.** Rollup table, incremental aggregate
   maintained at ingest, or a SQL-side rewrite (window functions for turn
   gaps)? Turn-gap logic spans provider/chain ordering — is it expressible
   in SQL exactly, or does exactness require ingest-time computation?
3. **Rollup freshness for the "live" tail.** token_hourly only covers >30d;
   here the hot window IS the last 30d. Hybrid read (rollup for closed
   hours + raw scan for the open hour) seems necessary — acceptable?
4. **Connection pool shape.** One reader per subsystem (like model
   analytics), a small pool, or a single shared reader? Does WAL + multiple
   readers + one writer need `wal_autocheckpoint` tuning given the 205 MB
   WAL finding?
5. **Where to run ANALYZE.** Startup is budget-sensitive (principle 3);
   post-migration is rare; the manual compaction path exists. Which owns
   it, and is `PRAGMA optimize` on connection close enough?
6. **Skills breakdown all-time** — intentional product choice or a bug?
   Needs a product decision at the clarify gate.
7. **Refresh cadence product floor.** Is a coalesced ≥5s refresh during
   active ingest acceptable for the "live instrument" feel (DESIGN.md), or
   does the hero band need a faster lane fed by a cheap query?
8. **Comparison-range UX.** Code insights deltas inherently need the prior
   period (range × 2, not next-larger-range). Is range×2 the intended
   semantics? Trends is documented as week-over-week — does it actually
   need 30d?
9. **Backfill risk.** Rolling up 4.2M rows on first run: chunked in
   background with progress (like retention delete's chunking)? What's the
   interrupted-mid-backfill story (principle 4)?
10. **Does the ingest explosion itself need a look?** 96% of model
    observations in 30 days may be expected (heavy use) or may indicate
    over-observation/duplication upstream — worth a bounded check before
    sizing rollups. (CPA polling work is active on this branch;
    interaction unknown.)
11. **Test authorization.** Constitution 7 requires explicit user sign-off
    for new automated tests; this feature is hard to trust without
    rollup-consistency and concurrency tests. Ask at clarify.

## Spec Review

Six parallel review passes (requirements, gaps, ambiguity, feasibility,
scope, stakeholders). All code claims in the spec were verified against the
live tree; the review found one factual error (retention DOES prune
`model_usage_observations` — corrected in Constraints above). Cross-
dimension findings merged; dimension tags noted per item.

### Critical Questions (answer before planning)

1. **Rollup correctness design is harder than the token_hourly analogy
   suggests — settle the model before migration N (one-way door).**
   Four compounding facts the spec under-specifies: (a) retention prunes
   `model_usage_observations` with no retention aggregate written, so
   post-prune the rollup is the only record — "raw is source of truth" and
   Goal 6's consistency check break; (b) `model_observation_sources`
   suppression state mutates retroactively (UPDATEs at storage.rs:5463,
   9000-9271; deletes at :2256), invalidating arbitrary rollup hours —
   needs source-keyed rollup rows or a dirty-hour queue; (c) the overview's
   DISTINCT (provider, analytics_session_id) counts don't decompose across
   hour buckets — rollup grain needs a session/source dimension or those
   facets fall back to 4M-row raw scans (and 30d IS worst case here, so
   fallback is a trap); (d) unlike token_hourly, raw rows stay, so the
   closed-hour/open-hour exclusivity watermark and its crash consistency
   must be defined. Also define what "numerically identical" means
   (bit-exact vs epsilon; closed buckets vs open tail).
   — flagged by: feasibility, gaps, ambiguity, requirements

2. **S2's parity criterion is unsatisfiable as written — redefine
   runtime-stats semantics before choosing a mechanism.** The current walk
   is now-dependent (tool-wait ceiling clamps to min(prev+6h, now),
   storage.rs:17303-17306) and window-relative (turns truncate at range
   start), so output is f(data, window, now) — no rollup can be
   "numerically identical" across runs. Also, re-ingest DELETEs and
   re-inserts session_events per source (storage.rs:17888), so incremental
   aggregates need per-source invalidation. And the 200ms-at-90d budget
   rules out the SQL-rewrite option (still scans ~2.9M index rows; expect
   2-5x, not 10x). Decide: pin `now` on a frozen corpus for parity
   testing, time-invariant semantics redefinition, and the aggregation
   mechanism — at plan time, not mid-build.
   — flagged by: feasibility, ambiguity

3. **Define one reproducible measurement protocol and complete the budget
   table.** "Cold" (app-cache miss vs fresh launch vs dropped OS page
   cache — order-of-magnitude difference at 13.5 GB), per-query vs
   per-view-render for the 500ms/200ms budgets, pinned DB snapshot +
   fixed window endpoints (live DB mutates daily; a frozen copy needs
   ~14-27 GB disk — where?), whether the "frozen retention corpus"
   fallback is 13.5 GB-scale, budgets (or explicit best-effort exemption,
   per principle 10) for the S5 cleanups and for Usage/Charts/Context view
   loads, and a contention budget for the fast query class under S3's
   5s-injection test.
   — flagged by: requirements, ambiguity, feasibility

4. **Maintenance concurrency and rollup lifecycle need a coordination
   story.** Backfill of 4.2M rows vs the ingest-quiesce lease
   (lib.rs:110-151), user-triggered retention prune, and VACUUM (which
   fails busy under long-lived readers — the existing compaction feature
   breaks without a close-readers contract); disk/WAL preflight and
   checkpoint strategy for backfill (house precedent: retention
   checkpoints per chunk via wal_checkpoint TRUNCATE); a bounded ingest
   write-path budget for ingest-time rollup maintenance; what Models
   shows mid-backfill (partial numbers violate principle 1; the raw 20s
   path violates S1); and post-build observability — staleness detection,
   error surfacing beyond log::warn, and a rebuild-rollup-from-raw
   command. Note the 013 cache's high-water probes are structurally blind
   to rollup UPSERTs (max-only scalars, storage.rs:203-229) — composition
   needs a monotonic updated_at, an explicit cache clear on rollup write,
   or a rollup-aware probe.
   — flagged by: gaps, stakeholders, ambiguity, requirements

5. **Declare the MVP cut and the reader-pool scope.** S1+S2 eliminate
   ~95% of measured pain (Models already runs on its own reader
   connection); the view-wide connection pool, refresh-cadence redesign,
   frontend cache re-architecture, and ANALYZE are candidates for
   descoping to follow-up slices sized against post-S1/S2 re-measurement.
   Decide the slice cut (proposed: A=model rollup, B=runtime stats,
   C=readers for still-slow paths + move CPU off the lock, D=refresh
   honesty/frontend cache, E=query cleanups) and — for whichever reader
   work ships — enumerate which callers migrate (widget only, or
   Manage/settings surfaces sharing the same queries), since a partial
   migration leaves unmigrated surfaces still blocked and ANALYZE changes
   plans app-wide (code today relies on a stat-free planner,
   storage.rs:17362-17366).
   — flagged by: scope, stakeholders, feasibility, ambiguity

6. **Resolve open question 10 (CPA interaction) FIRST, plus the product
   decisions at this gate.** (a) The 96%-in-30d ingest explosion may be
   upstream over-observation — CPA polling is in-flight on this very
   branch, writes near model observation tables, and collides on the
   36-to-N schema bump; a bounded check gates rollup sizing and schema
   design. (b) Skills breakdown all-time: bug or product choice?
   (c) Refresh-cadence floor: coalesced 5s vs DESIGN.md live-instrument
   feel — note this explicitly SUPERSEDES 013's recorded clarification Q7
   ("live ingest events still force a refresh"), as does the rollup vs
   013's temp-table decision. (d) Comparison-range semantics: range-times-2
   (prior period) vs next-larger preset — and reconcile Goal 4's "lazily
   beyond" clause with S4's strict no-over-range AC (currently
   contradictory).
   — flagged by: scope, stakeholders, ambiguity

7. **Test authorization (constitution 7).** Rollup-consistency,
   quiesce/concurrency, and the 5s-injection contention test are the
   specific asks — without them S1/S3/Goal 6 acceptance is only manually
   demonstrable, and the durable verification artifact (committed harness
   output, per 013 precedent) must be named if tests are declined.
   — flagged by: requirements, feasibility, stakeholders, scope

### Non-Blocking Observations

- Budgets are machine-relative (one dev machine); state so, and
  sanity-check backfill duration on slow disks — that's where today's 20s
  stall hurts most. Fresh-install/small-DB path must be a no-op
  (instant-complete backfill, no rollup overhead regression).
- Estimate and document added disk footprint (rollup tables + sqlite_stat1
  + WAL growth) for a user already carrying 13.5 GB + 205 MB WAL + 2.5 GB
  .bak; give the .bak "user-communication item" an owner (follow-up bead
  at create-beads).
- Migration N should be additive DDL only, backfill out-of-band (013
  precedent); downgrade = hard SCHEMA_TOO_NEW refusal (storage.rs:4130) —
  acknowledge as accepted UX.
- Bucketing is UTC everywhere today (storage.rs:9614, 15629); record
  "buckets are UTC" as an explicit decision and prefer hourly over daily
  keys (daily bakes the timezone in permanently).
- Enumerate at plan time: Manage/settings surfaces calling the touched
  commands; superseded paths removed vs kept (temp-table path, INDEXED BY
  pin, potentially-redundant model indexes — 3.6 GB); add non-goals for
  post-rollup index cleanup and rollup-then-prune-raw asks (predictable
  day-after requests). Cite 013's session-breakdown deferral (S6/OQ6) for
  decision continuity.
- 013's cached-only endpoints (get_all_bucket_stats,
  get_context_savings_analytics) have unmeasured cold cost at 13.5 GB —
  measure or explicitly exclude. Add a warm-path regression check (warm
  no slower than current warm) to guard the 013 cache composition.
- S4's "no query window exceeds R" needs cheap query-window logging to be
  checkable; name it as a deliverable. dbstat vtab is not confirmed in the
  bundled SQLite — don't depend on it. RFC3339-TEXT lexicographic ordering
  is load-bearing (storage.rs:17376); one confirmation grep at plan time.

## Clarifications

Human answers recorded 2026-08-02 (all recommended options accepted).
Acceptance criteria in the body have been amended to match.

**Q1: Model-observations rollup correctness model?**
A: **Source-keyed hourly rollup.** Rows keyed
`(hour_utc, provider, derived_model_id, source_key)`; suppression flips,
source deletion, and re-ingest invalidate exactly that source's rollup
rows. Hybrid read: closed hours from rollup + open hour from raw. For
retention-pruned windows the rollup becomes the authoritative record
(supersedes "raw is source of truth" for those windows; principle 1 is
served by exact fold-then-prune ordering). Buckets are UTC, hourly —
never daily keys.

**Q2: Runtime-stats semantics?**
A: **Redefine to time-invariant semantics.** Turn values are finalized at
ingest (closed turns are pure functions of the data); only open/live turns
are computed at query time against `now`. Parity is verified on a frozen
corpus with pinned `now`. The ≤200ms@90d budget applies to the
aggregated path; the SQL-rewrite-only option is rejected.

**Q3: Measurement protocol?**
A: **Approved as proposed.** Frozen snapshot copy of the live 13.5 GB DB
is the benchmark corpus (~14 GB disk required); "cold" = first call
in-process with app-level caches bypassed (OS page cache uncontrolled but
recorded); budgets are per-query, plus per-view cold render budgets for
Usage/Charts/Context to be set in plan; S5 cleanups get hard budgets
(starting point: `get_session_breakdown` ≤300ms @30d,
`get_code_stats_history` ≤300ms @30d — plan may tighten with rationale).
Fast-class queries under the S3 5s-injection test: ≤100ms p95.

**Q4: Backfill & maintenance coordination?**
A: **Approved as proposed.** Chunked backfill on the writer connection
(bounded per-chunk transactions, per-chunk WAL checkpoint, disk
preflight, resumable bookmark), yields to the ingest-quiesce lease
(prune/VACUUM win); during the one-time backfill the Models view keeps
the existing raw path with a "building index" progress note; a
rebuild-rollup command lands in the settings Performance tab; rollup
writes bump a monotonic marker the 013 cache probes can see.

**Q5: MVP cut?**
A: **Slices, re-measured.** A = model rollup (S1), B = runtime stats
(S2) ship first; C = readers for still-slow paths + move CPU off the
lock (S3), D = refresh honesty + frontend cache (S4), E = query cleanups
(S5) are beaded now but gated on post-A/B re-measurement to size (or
shrink) them against the new baseline.

**Q6: Product decisions?**
A: (a) **Yes** — check ran 2026-08-02; results: no duplication (COUNT =
COUNT DISTINCT = 4,201,089; unique index holds), no backfill-timestamp
artifact (`observed_at_ms` is transcript time), volume is genuine
fleet-scale Codex usage at per-`token_count` granularity (~66% of rollout
lines; ~745 rows/MB; bursts to ~866k rows/day from as few as 77 files,
quiet-day floor 5-10k/day). Per-token_count fidelity is the intentional
"replayable evidence" contract (lat.md/data-flow.md:131-155) — the rollup
layer, not a parser change, is the right compression point. **Size the
rollup for burst days up to ~1M rows/day, not the 135k/day average.**
CPA modules write neither `model_usage_observations` nor
`usage_snapshots` — no schema collision. SEPARATE BUG found (own bead,
outside this feature): post-2026-07-28 source-admission regression —
new rollouts on disk (0.2-1.3 GB/day) are mostly never enumerated for
ingest (5-8 sources/day admitted vs 100-200 during the burst; a 27 MB
rollout with 18,057 token events has zero DB rows and no inventory row
despite live ingest); true current throughput understated ~100×. (b) Skills all-time is a
**bug** — scope to the selected range. (c) Refresh cadence during active
ingest: **coalesce to ≥5s** (normative; explicitly supersedes 013
clarification Q7's "live ingest events still force a refresh").
(d) Comparison deltas use the **prior period of equal length (range×2)**,
replacing next-larger-preset; the prior-period fetch is the one
enumerated, justified over-range query.

**Q7: Test authorization?**
A: **Authorized** (constitution 7): rollup-consistency tests,
backfill/quiesce concurrency tests, and the 5s-injection contention
test, each linked one-to-one with lat.md specs.
