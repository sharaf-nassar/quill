# Plan: analytics-query-perf

Implementation plan for the clarified spec (all 7 Clarifications binding,
A-options chosen; Spec Review feasibility corrections binding). Scope per
Q6: MVP = S1–S4 + S5's migration-drop, plus a separate "Compact database"
operation (VACUUM). S6 deferred entirely. S7 endpoints get caching only,
no query rewrites. Per-command timing logs ship with the feature (Q4).

Grounding facts verified against the worktree (identical to main checkout):

- `src-tauri/src/storage.rs`: `Storage { conn: Mutex<Connection>, db_path }`
  — one process-wide connection mutex. `MAX_SUPPORTED_SCHEMA_VERSION: i32 =
  33` at line 58; `SCHEMA_TOO_NEW` refusal at ~3611. Migrations are
  `if current_version < N { tx … INSERT INTO schema_version VALUES (N) }`;
  v33 (last) at ~5519. `tool_actions_legacy_v30` is created by the
  migration-30 rename at ~5048 (`ALTER TABLE tool_actions RENAME TO
  tool_actions_legacy_v30`). `get_model_usage_overview` (~6422) sets
  `range_end = Utc::now()` (a sliding window), opens a **fresh** read-only
  connection via `open_model_analytics_reader` (~5615), then builds a
  `CREATE TEMP TABLE scoped_overview` + two temp indexes on every call.
  `get_model_analytics` (~6097) and `get_model_history` (~7317) re-scan the
  same range. Model backfill worker (~5648) writes observations through the
  **main** `self.conn.lock()`.
- `src-tauri/src/lib.rs`: analytics IPC handlers ~2311–2950 all use
  `tauri::async_runtime::spawn_blocking`. In-process cache precedent
  `usage_cache()` at 1539 is a `static OnceLock<Mutex<Option<
  UsageCacheEntry>>>` with a `refreshed_at: DateTime<Utc>` + key — the
  model for the analytics cache. Ingest events are emitted **outside** the
  app for hook writes: `tokens-updated` (`server.rs:301`),
  `sessions-index-updated` (`sessions.rs:1541`, `server.rs:774/793`),
  `transcript-analytics-updated` (const `lib.rs:76`, emitted at 499/683/
  2862+). The HTTP ingest server and separate widget/hook processes commit
  to SQLite on connections the app does not own.
- Frontend `src/hooks/`: `useModelAnalytics.ts` already implements the
  target pattern — `scopeCacheRef: Map<identity, ScopeCacheEntry>`,
  `SCOPE_DEBOUNCE_MS = 200`, `requestGenerationRef` generation guard
  (request-id/abort), `EVENT_COALESCE_MS = 1000`, stale-while-revalidate
  (`initialLoading = retainedData === null; refreshing = retainedData !==
  null`). Duplicate invokes confirmed: `get_llm_runtime_stats` in
  `useLlmRuntimeStats.ts:25` **and** `useCodeInsights.ts:159/165`;
  `get_snapshot_count` via `useAnalyticsData.ts:110`, instantiated twice
  because `AnalyticsView.tsx` `SnapshotGate` calls
  `useAnalyticsData(null, "24h")` alongside the tab hook;
  `get_token_hostnames` in `useTokenData.ts:63` is range-independent yet
  refetched on range change.
- Test layout: Rust has inline `#[cfg(test)] mod tests` in `storage.rs`
  (~16442) plus integration tests under `src-tauri/tests/`. Frontend has
  **no** test runner (only `tsc --noEmit` + `eslint`); a dev-only IPC mock
  harness exists (`src/mocks/ipcFixtures.ts`, `installBrowserMock.ts`, wired
  in `main.tsx:17`).

---

## Architecture Approach

Three independent workstreams that share one new backend cache primitive
and one new frontend cache primitive, plus a standalone DB-maintenance
path. Each targets a distinct spec story cluster and can land behind the
others without coupling.

**1. Backend result cache (S3, S7-cache).** Add a single in-process cache
module modeled on `usage_cache()` — a `static OnceLock<Mutex<HashMap<
CacheKey, CacheEntry>>>` living in `storage.rs` (or a new
`storage/analytics_cache.rs` submodule). `CacheKey = (command: &'static
str, range, provider: Option<String>, time_bucket: i64)`. `CacheEntry`
stores the serialized payload (or the typed response `Arc`), an
`inserted_at: Instant`, and a captured **data-version fingerprint** per
source table. On read: if `now - inserted_at > TTL` (30–60 s, config
const) → miss; else re-probe the source tables' fingerprint and compare —
match serves the cached value, mismatch is a miss. This satisfies Q2
(key = command,range,provider,time-bucket), Q3 (per-table version + TTL
cap), and the Spec-Review #2 sliding-window correction (the `time_bucket`
dimension quantizes `Utc::now()` so a wall-clock advance past the bucket
boundary is a natural miss).

*Data-version probe — decision.* `PRAGMA data_version` is **rejected**:
per SQLite semantics it is a per-connection counter that only reflects
commits by *other* connections and does not change for same-connection
writes, and `get_model_usage_overview` opens a **fresh** connection every
call, so the counter has no stable baseline to compare against across
calls, processes, or restarts. Instead use an **absolute per-table probe**:
`SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM <table>` (or
`MAX(observed_at_ms)` where a monotonic ingest column exists, e.g.
`model_usage_observations`). Absolute values are comparable across
connections, across processes (hook/widget/HTTP-server writers), and across
the in-process backfill worker (which commits on the main connection), and
survive restarts. `COUNT(*)` catches deletes (`cleanup_old_observations`);
`MAX(rowid)`/`MAX(observed_at_ms)` catches inserts. The probe is one
indexed aggregate per source table per cache read — cheap relative to the
work it guards. A spike (see Risks) confirms probe cost and cross-process
observability before wiring.

*Probe failure — fail-open.* Any probe error (e.g. `SQLITE_BUSY` while a
concurrent VACUUM holds the file lock) is treated as a **cache miss**:
compute the result fresh, log at `warn`, and never serve the stale cached
value on a probe failure. Fail-open, never fail-stale. Separately, a cached
command whose *own* query hits a real error propagates that error to the
caller exactly as it does today — the cache never converts a genuine query
failure into a silent stale serve.

Alternatives rejected: (a) *persistent `analytics_cache` SQLite table with
data-version stamps* (Open Q2 option B) — survives restarts but adds a
write path, a migration, and its own invalidation surface; the audit shows
warm hits dominate within a session, so memory is sufficient and simpler.
(b) *`PRAGMA data_version` on a held probe connection* — would work but
requires owning a long-lived connection purely for probing and still can't
distinguish tables; absolute probes are strictly more informative. (c)
*Invalidate-all-on-any-event* (Open Q3 option A) — Spec Review #3 notes it
collapses the cache to a passthrough under active ingest, exactly when
users watch Now; per-table versions avoid that.

**2. Frontend cache/debounce/dedupe (S1, S2, S4).** Extract the
`useModelAnalytics` machinery into a shared `useCachedInvoke` hook
(`src/hooks/useCachedInvoke.ts`) — Open Q4 option A (shared utility, chosen
to prevent drift). It owns: the identity-keyed `Map` cache with
stale-while-revalidate, the 200 ms debounce (first request immediate,
subsequent debounced), the `requestGenerationRef` guard (request-id/abort:
stale responses discarded when `generation !== current`), and a **per-hook
event-subscription** helper (existing 1 s coalesce). The invariant: each
ported hook subscribes **only** to the ingest events for the tables its
commands read, so an event must **never** revalidate a hook that does not
read its tables. The explicit event→hook map the layer wires:

| Ingest event | Tables it signals | Hooks that subscribe |
|--------------|-------------------|----------------------|
| `tokens-updated` | `token_snapshots`, `model_usage_observations` | `useTokenData`, `useLlmRuntimeStats`, `useModelAnalytics` |
| `transcript-analytics-updated` | transcript / context-savings source tables | `useContextSavingsStats` |
| `sessions-index-updated` | session index | `useSessionHealth`, `useActivityPattern`, `useAnalyticsData` |

The remaining hooks (`useCodeStats`, `useCodeInsights`, `useBreakdownData`)
subscribe to whichever of these events signal the tables their commands
actually read, confirmed per hook during the port — default to **no**
subscription rather than over-subscribing, so no event triggers an
unrelated revalidation. Refactor `useModelAnalytics` onto it first
(behavior-preserving
— it is the reference), then port `useTokenData`, `useCodeStats`,
`useCodeInsights`, `useLlmRuntimeStats`, `useContextSavingsStats`,
`useSessionHealth`, `useActivityPattern`, `useBreakdownData`,
`useAnalyticsData`. Dedupe work: hoist `get_token_hostnames` out of the
range-keyed fetch (fetch once, range-independent); collapse the two
`get_llm_runtime_stats` callers and the two `get_snapshot_count` callers.
Resolve Open Q8 by **lifting to the common parent**: a single
`get_snapshot_count` fetch in `AnalyticsView` shared down to both
`SnapshotGate` and the tab hook via props/context, so the gate no longer
issues its own count. Reuse `useCodeInsights`'s current-window series from
the comparison-range fetch: when that series is already cache-warm, the
comparison cards issue **0 new overlapping history queries** — they reuse
≥1 already-fetched series instead of refetching the shared slice (the code
already flags this at `useCodeInsights.ts:85`).

Alternative rejected: *copy the pattern per hook* (Open Q4 option B) —
fewer files touched but guarantees drift across nine hooks; the shared hook
is the whole point of "generalize the pattern."

**3. DB maintenance (S5).** Split into two deliverables per Q1. (a)
**Migration v34** drops `tool_actions_legacy_v30` only (`DROP TABLE IF
EXISTS`, idempotent), bumps `MAX_SUPPORTED_SCHEMA_VERSION` to 34, no VACUUM
inside the migration — it must stay fast and cannot block startup. (b) A
separate **"Compact database"** operation (`compact_database` IPC command),
**user-triggered only** (idle-trigger scoped OUT of MVP — button-only),
runs `VACUUM` on a **dedicated maintenance
connection** with ingest quiesced, a preflight free-disk check (~2× file
size), progress events, and skip-and-report on any failure. Critically,
`DROP TABLE` in WAL mode reclaims **zero** OS bytes (Spec Review #1) — only
the VACUUM shrinks the file, so the two deliverables are complementary, not
redundant.

**4. Timing logs (Q4).** A lightweight `log::info!`-level per-command timing
wrapper around the analytics IPC handlers in `lib.rs` (elapsed ms +
command + range + cache hit/miss), gated so it is cheap in release. No CI
perf harness this round; the logs make before/after measurable on the real
7.45 GB DB.

---

## Affected Components

Backend (`src-tauri/src/`):

- `storage.rs`
  - **New migration v34** appended after the v33 block (~5570): `if
    current_version < 34 { tx.execute_batch("DROP TABLE IF EXISTS
    tool_actions_legacy_v30;"); INSERT INTO schema_version VALUES (34) }`.
  - **`MAX_SUPPORTED_SCHEMA_VERSION`** (line 58): `33 → 34`. This is the
    one-way door (Spec Review #5): a v34 build stamps the DB and older
    builds hard-refuse via the `SCHEMA_TOO_NEW` check (~3611).
  - **New analytics cache primitive** (new `impl`/module): key type, entry
    type, per-table probe fn, `get_or_compute` helper. Wraps the bodies of
    `get_model_usage_overview` (~6422), `get_model_analytics` (~6097),
    `get_model_history` (~7317) for S3, and `get_all_bucket_stats`
    (~9053 via `lib.rs:2398`) + `get_context_savings_analytics` (~9558 via
    `lib.rs:2927`) for S7-cache. The overview's temp-table body is left
    intact for now (Open Q7: cache the result, benchmark before replacing
    materialization).
  - **New `vacuum_database` / maintenance method** using a dedicated
    connection (not `self.conn`), preflight disk check, quiesce hook.
- `lib.rs`
  - **New `compact_database` IPC command** (register in the
    `tauri::generate_handler!` list) emitting progress events.
  - **Per-command timing wrapper** applied to the analytics handlers
    (2311–2950).
  - Ingest quiesce coordination: a process-wide "maintenance in progress"
    guard the ingest write paths honor (see Risks — ingest quiescing).
- `server.rs` / ingest write paths: consult the quiesce guard so writes are
  paused (or fail-soft-and-retry) during VACUUM rather than hitting
  `SQLITE_BUSY`.

Frontend (`src/`):

- `src/hooks/useCachedInvoke.ts` — **new** shared hook (extracted pattern).
- `src/hooks/useModelAnalytics.ts` — refactor onto `useCachedInvoke`
  (reference implementation; behavior preserved).
- Ported hooks: `useTokenData.ts`, `useCodeStats.ts`, `useCodeInsights.ts`,
  `useLlmRuntimeStats.ts`, `useContextSavingsStats.ts`, `useSessionHealth.ts`,
  `useActivityPattern.ts`, `useBreakdownData.ts`, `useAnalyticsData.ts`.
- `useTokenData.ts` — hoist `get_token_hostnames` (line 63) out of the
  range-keyed fetch.
- `useCodeInsights.ts` — reuse current-window series from the comparison
  fetch (the existing `:85` note); drop its private `get_llm_runtime_stats`
  call in favor of the shared cached result.
- `src/components/analytics/AnalyticsView.tsx` — `SnapshotGate` (~53) stops
  instantiating a second `useAnalyticsData(null, "24h")`; Open Q8 resolved
  by lifting a single `get_snapshot_count` fetch into `AnalyticsView` and
  sharing it to both the gate and the tab hook via props/context.
- `src/components/analytics/NowTab.tsx` — consume deduped hooks; the Now
  tab's stale-while-revalidate keeps live ingest events forcing a refresh
  (Q7). No Now-tab special case: the single shared TTL constant governs it
  like every other tab.
- New "Compact database" UI affordance — a **button only** (idle-trigger is
  out of MVP) plus progress and skip-report states — minimal, in the
  existing settings/systems surface; wires to `compact_database` and renders
  its progress events. (No redesign per Non-Goals.)
- `src/mocks/ipcFixtures.ts` — add `compact_database` fixture + progress
  event stubs so the dev mock harness renders the new flow.

---

## Data Model

No new **persistent** cache table (Q2 — in-process memory only). Schema
change is a single destructive drop of a provably-dead table plus the
version bump.

- **Migration v34**: `DROP TABLE IF EXISTS tool_actions_legacy_v30;`
  Idempotent — the guard is `IF EXISTS`, and the `current_version < 34`
  gate makes it run once. On DBs where the table never existed (fresh
  installs never ran the migration-30 rename against pre-existing data) or
  was already dropped, it is a no-op. Recorded via `INSERT INTO
  schema_version (version) VALUES (34)` inside the migration transaction,
  matching every prior migration's shape.
- **`MAX_SUPPORTED_SCHEMA_VERSION`**: `33 → 34` (storage.rs:58). One-way
  door accepted and documented (Q5): single-user local app, the legacy
  table is provably dead, no backup gate.
- **In-process cache structures** (not persisted, no schema):
  - `CacheKey { command: &'static str, range: <RangeEnum>, provider:
    Option<String>, time_bucket: i64 }` — `time_bucket = Utc::now()
    .timestamp() / BUCKET_WIDTH_SECS` quantizes the sliding window so a
    boundary crossing is a cache miss (fixes Spec Review #2).
  - `CacheEntry { payload, inserted_at: Instant, versions: TableVersions }`.
  - `TableVersions` = per-source-table `high_water: i64` from
    `COALESCE(MAX(rowid)|MAX(observed_at_ms), 0)`. The probe uses an indexed
    maximum only because the cache-probe spike found `COUNT(*)` too costly;
    the TTL bounds delete staleness. Source-table sets differ per command
    (model commands probe `model_usage_observations` +
    `model_observation_sources`; token/bucket commands probe
    `token_snapshots`; context-savings probes its source tables) so
    per-table granularity holds (Q3: context-savings ingest does not
    invalidate the model cache).
  - `TTL_SECS` const (30–60 s) — hard staleness cap when an ingest event is
    missed (Q3), and the definition of "stale" that makes S1's
    background-refresh criterion testable (Spec Review #3).

VACUUM changes no schema; it only rewrites the file to reclaim freelist +
the dropped table's ~0.87 GB. Decision to defer `auto_vacuum=INCREMENTAL`
(it needs one full VACUUM to take effect anyway, Spec Review #1) — note it
as a follow-up, not MVP.

---

## API / Interface Changes

IPC (Tauri commands, `tauri::generate_handler!` in `lib.rs`):

- **`compact_database` — new command.** Runs the maintenance path: preflight
  free-disk check (~2× current file size), quiesce ingest, VACUUM on a
  dedicated connection, un-quiesce. Returns a structured result
  (`{ status: "completed" | "skipped", reason?, bytes_before, bytes_after }`)
  and never risks silent data loss — any failure (insufficient disk,
  interruption, busy) is a skip-and-report branch.
- **New progress events** emitted during compaction, e.g.
  `compact-database-progress` (`{ phase, pct? }`) and
  `compact-database-finished` (`{ status, bytes_before, bytes_after }`), so
  the UI can show a "compacting" indicator and the final delta. (VACUUM
  gives no native progress callback; phases are coarse:
  `preflight → quiescing → vacuuming → done`.)
- **No signature change** to the cached analytics commands
  (`get_model_usage_overview`, `get_model_analytics`, `get_model_history`,
  `get_all_bucket_stats`, `get_context_savings_analytics`) — caching is
  internal; same request args, same response shape. Cache hit/miss is
  surfaced only via the timing log.
- **Timing-log surface**: `log::info!` lines of the form
  `analytics_cmd={name} range={r} provider={p} cache={hit|miss} elapsed_ms={n}`
  from the IPC wrapper. Not an IPC contract; read from app logs / stderr.

**Breaking change (documented, Q5 / Spec Review #5):** migration v34 bumps
`MAX_SUPPORTED_SCHEMA_VERSION` to 34. Once any v34 build opens the DB the
schema is stamped 34 and **older builds hard-refuse** it (`SCHEMA_TOO_NEW`).
Combined with the irreversible `DROP TABLE` and (post-compaction) VACUUM,
there is no downgrade path. Accepted: single-user local app, dead table.
Document in the migration comment and release notes. **Non-goal precedent
guard** (Spec Review, non-blocking): the VACUUM lives in
`compact_database`, **not** inside the migration — so future migration
authors are not taught that blocking compaction belongs in a schema step.

Frontend interface: `useCachedInvoke` is an internal hook API (not IPC).
Public analytics hook return shapes are preserved (they already expose
loading/refreshing/data/error via `useModelAnalytics`'s pattern); consumers
gain background-revalidate behavior without prop changes.

---

## Testing Strategy

Per the repo's rules, tests are **specified here for later approval**, not
pre-written. They respect the existing layout: Rust unit tests in
`storage.rs`'s inline `#[cfg(test)] mod tests` and integration tests under
`src-tauri/tests/`. **Frontend has no test runner today** — only `tsc
--noEmit` + `eslint` — so a runner is a prerequisite decision (below).

Backend (Rust — inline `#[cfg(test)]` / `src-tauri/tests/`):

- **Migration v34 idempotency (S5).** Open a temp DB with
  `tool_actions_legacy_v30` present → migrate → assert the table is gone and
  `schema_version` max is 34. Re-run migration on the already-migrated DB →
  no error (idempotent). Open a DB where the table never existed → no error.
- **`MAX_SUPPORTED_SCHEMA_VERSION` gate (S5 / one-way door).** A DB stamped
  35 is refused with `SCHEMA_TOO_NEW`; a DB stamped 34 opens. (Mirrors the
  existing gate tests at ~21701.)
- **Cache hit/miss + TTL (S1/S3).** Populate observations; call a cached
  command twice → second is a hit (assert via an injected clock or a
  hit-counter). Advance the injected clock past TTL → miss.
- **Per-table version invalidation (S3/Q3).** Cache a model command; insert
  into a **non-model** source table → model cache still hits (per-table
  granularity). Insert into `model_usage_observations` → model cache
  invalidates (probe mismatch). Simulate an **out-of-process** write by
  committing on a *second* connection to the same file → probe still
  detects it (the cross-connection correctness that justifies absolute
  probes over `data_version`).
- **Probe failure fails open (S3).** A probe that raises (inject a
  `SQLITE_BUSY`/error on the probe query) → treated as a miss: the command
  recomputes fresh and returns a correct result with **no user-facing
  error**, and the stale cached value is never served. Assert a `warn` log
  is emitted. Separately, a cached command whose own query raises propagates
  that error to the caller (no silent stale serve).
- **Sliding-window bucket (Spec Review #2).** Same `(range, provider)` but a
  `time_bucket` boundary crossing → miss (no stale-boundary serve).
- **Cache-probe cost spike (Risks).** A `#[ignore]` benchmark test /
  criterion-free timing assertion documenting probe latency on a large
  fixture, so the probe's guard cost is known. The spike uses a 250,000-row
  SQLite fixture shaped like the model observation fact table and compares
  five warm absolute probes against a representative filtered aggregate. It
  also proves an independent connection's committed insert changes the
  fingerprint. The measured `COUNT(*)` probe exceeded the 5% budget, so the
  selected implementation uses `MAX(observed_at_ms)` only and requires that
  it consume <5% of guarded aggregate time. The TTL cap, rather than the
  probe, detects deletes.
- **VACUUM path (S5).** Preflight rejects when free disk < 2× file (mock the
  disk check) → returns `skipped` with reason, DB untouched. Happy path on a
  small temp DB → `bytes_after < bytes_before` after dropping a table.
  Quiesce guard blocks a concurrent write attempt during the run.
- **`get_all_bucket_stats` / `get_context_savings_analytics` caching
  (S7-cache).** Because the `get_all_bucket_stats` rewrite is deferred, its
  bound is exactly **"served from cache on repeat within TTL"** — a second
  call inside the TTL window is a cache hit (no recompute); the cached result
  equals the uncached computation (correctness under caching, no query
  rewrite).

Frontend — **decision (no runner added this round).** No test runner
exists, and repo rule forbids writing test code without an explicit
request. So each hook-port item's definition of **"done"** is: `npm run
typecheck` (`tsc --noEmit`) green + `npm run lint` green + **manual
verification against the existing dev IPC mock harness**
(`src/mocks/ipcFixtures.ts` / `installBrowserMock.ts`) — exercise the SWR,
race, debounce, and dedupe behaviors by hand in the mock. Adding `vitest` +
`@testing-library/react` + `jsdom` is a **separate follow-up bead** that
requires explicit user approval (new deps → "Missing Tools" rule; test code
→ explicit-request rule). The automated SWR/race/debounce/dedupe specs below
move to that follow-up bead and do not gate this feature:

- **Stale-while-revalidate (S1).** Revisiting a cached range renders cached
  data with `initialLoading === false` immediately and issues a background
  refetch; fresh data swaps in.
- **Request-race guard (S2).** Fire 4 rapid range switches (mocked invoke
  with staggered resolution) → only the final range's data renders; an
  earlier-range late resolution is discarded (generation guard).
- **Debounce (S2).** 4 switches within 200 ms → one settled backend call.
- **Dedupe (S4).** One Now switch issues `get_llm_runtime_stats` once and
  `get_snapshot_count` once (spy on the mock invoke); `get_token_hostnames`
  is not called on a range change.

Until that follow-up lands, the four behaviors above are checked manually in
the dev mock as part of each port's "done" bar.

Perf measurement (Q4): not a gate — capture the timing logs on the real
7.45 GB `usage.db` before/after for the wide-range Models switch and one
Now-tab switch, and record the numbers against the guidance targets
(~300 ms warm / ~1.5 s cold).

---

## Risks

- **VACUUM blocks every IPC via the single connection mutex (Spec Review
  #1, highest risk).** The app holds one `Mutex<Connection>`; a minutes-long
  VACUUM on it freezes all reads and writes. *Mitigation:* run VACUUM on a
  **dedicated maintenance connection** opened for the operation (not
  `self.conn`), and quiesce ingest for the window so writers do not race it.
  The model-reader connection and any live read still contend on the file
  lock during VACUUM — accept a brief app-wide pause, gated behind explicit
  user/idle trigger with a progress indicator, never at startup. *Spike:*
  measure real VACUUM wall-time on a 7.45 GB copy before finalizing the UX.
- **Ingest quiescing mechanism.** Ingest writes arrive out-of-process (hook
  POSTs to `server.rs`, widget, backfill worker). A process-wide "maintenance
  in progress" flag the write paths check is required; out-of-process
  writers can only be *paused at the app's write boundary* (the HTTP
  server), and truly external writers must fail-soft-and-retry rather than
  drop data (the DB is the user's only history). *Mitigation:* set busy_timeout
  generously and have the HTTP ingest handler return a retriable status while
  quiesced; document that hooks retry. *Open validation:* confirm no writer
  path bypasses the app's server.
- **Cache-version probe reliability across processes (Spec Review #2).**
  If the probe cannot see out-of-process/backfill commits, the cache goes
  permanently stale. *Mitigation:* absolute per-table probes (`COUNT` +
  `MAX(rowid)`/`MAX(observed_at_ms)`) are read fresh each check and are
  connection/process/restart-agnostic; `data_version` was rejected for
  exactly this reason (fresh per-call connection = no stable baseline).
  *Spike:* a two-connection test proving a commit on connection B is
  observed by a probe on connection A; measure probe latency so the guard
  cost stays well under the work it saves.
- **Sliding-window correctness (Spec Review #2).** `range_end = Utc::now()`
  makes `(range, provider)` a moving target. *Mitigation:* the `time_bucket`
  key dimension + TTL cap bound drift; covered by a dedicated test.
- **TTL vs. live Now tab (Q7 / Spec Review #7).** Cache-first could make the
  live Now tab feel stale. *Resolution (Clarification Q7):*
  **stale-while-revalidate everywhere** with the **single shared TTL
  constant** — the Now tab gets **no special-case exemption** and no
  separate TTL. Its liveness comes from the always-on background refetch
  plus live ingest events forcing an immediate one; the short TTL bounds
  worst-case staleness for every tab uniformly.
- **Rollback story / one-way door (Q5 / Spec Review #5).** No downgrade
  after v34 + DROP + VACUUM. *Mitigation:* accepted and documented in the
  migration comment + release notes; the DROP is `IF EXISTS` and the table
  is provably dead, so forward-only risk is minimal. VACUUM is a separate,
  user-gated step, so a user who never compacts keeps a recoverable file
  until they opt in.
- **Frontend refactor regressions across nine hooks.** Porting to a shared
  hook risks subtle behavior changes. *Mitigation:* refactor
  `useModelAnalytics` onto `useCachedInvoke` first as a behavior-preserving
  proof, keep `tsc`/`lint` green each step, port one hook at a time.
- **Empty/fresh-install + zero-result states (Spec Review, non-blocking).**
  Caching an empty range and rendering no-data states must be exercised so
  the cache does not mask an empty result as "loading forever."
- **Follow-up honesty (Spec Review, non-blocking).** S5 drops 0.87 GB but
  `tool_actions` (1.69 GB) + `session_events` (0.77 GB) dominate; file the
  retention/pruning follow-up bead now (Q6) so S5's "disk usage drops" is
  not oversold.

---

## Sequencing

Ordered work items with explicit dependency edges. This becomes the bead
DAG. Names are descriptive; dependencies are stated inline.

- **Verify timestamp offset uniformity** (spike). One-time check on the real
  DB that stored RFC3339 timestamps use uniform offsets (Spec Review,
  non-blocking) so any lexicographic pruning assumptions hold. *Blocks:
  nothing in MVP directly (S6 deferred) but informs cache probe column
  choice.* Independent — run first, cheap.

  **Result (2026-07-24):** Passed. A read-only scan of the real
  `~/.local/share/com.quilltoolkit.app/usage.db` found every stored RFC3339
  value with an explicit offset uses `+00:00`: `observations.timestamp`
  (212,475 rows), `token_snapshots.timestamp` (2,319),
  `usage_snapshots.timestamp` (67,837), `usage_snapshots.resets_at`
  (64,189), `token_hourly.hour` (169), `usage_hourly.hour` (1,014), and
  `learned_rules.created_at` (8). Other time-like columns currently use
  SQLite's offset-free default format rather than RFC3339. Lexicographic
  ordering is therefore safe for the RFC3339 values present in this DB;
  cache probes should still prefer absolute numeric/count values so future
  writers cannot invalidate that assumption.

- **Cache-probe reliability spike.** Prove absolute per-table probes detect
  a second-connection commit and measure probe latency on a large fixture.
  Note that `SELECT COUNT(*)` is a **full-table scan** in SQLite (not
  O(1)), so probe cost is not free. *Acceptance:* on the real fixture, a
  probe must cost **<5% of the guarded query's cost**; if `COUNT(*)`
  overruns that budget, fall back to **`MAX(rowid)`/`MAX(observed_at_ms)`
  only** probes (accepting that deletes are then caught by the TTL cap
  alone, not the probe). *Blocks: Backend cache primitive.* Independent
  start.

- **VACUUM wall-time + quiesce spike.** Measure VACUUM on a 7.45 GB copy and
  prototype the quiesce flag. *Blocks: Compact database command.* Independent
  start.

- **Backend cache primitive.** Implement `CacheKey` / `CacheEntry` /
  `TableVersions` / `get_or_compute` in `storage.rs`. *Depends on:
  Cache-probe reliability spike. Blocks: Cache model commands, Cache S7
  endpoints.*

- **Cache model commands (S3).** Wrap `get_model_usage_overview`,
  `get_model_analytics`, `get_model_history` in the cache; keep the temp
  table (Open Q7 — benchmark before replacing). *Depends on: Backend cache
  primitive. Blocks: nothing downstream; feeds timing measurement.*

- **Cache S7 endpoints (S7-cache).** Wrap `get_all_bucket_stats` and
  `get_context_savings_analytics`; no query rewrite. *Depends on: Backend
  cache primitive.*

- **Per-command timing logs (Q4).** Timing wrapper on analytics IPC handlers
  in `lib.rs`. *Depends on: nothing (can precede caching), but most useful
  after Cache model commands so hit/miss is loggable. Recommend after
  Backend cache primitive so the log carries cache status.*

- **Migration v34 drop + version bump (S5a).** Append the v34 migration,
  bump `MAX_SUPPORTED_SCHEMA_VERSION` to 34. *Acceptance (one-way door):*
  the bead **cannot close** until both the **migration comment** explaining
  the irreversible v34 + `DROP` + VACUUM path AND the **release-note text**
  documenting it are written. *Independent of the cache work. Blocks:
  nothing (the drop reclaims no bytes alone — VACUUM does that).*

- **Ingest quiesce guard + retriable ingest boundary.** The process-wide
  "maintenance in progress" guard in `lib.rs`, the `server.rs` HTTP ingest
  handler returning a **retriable** status while quiesced, and the backfill
  worker checking the guard before its `self.conn` writes. *Acceptance:* a
  write arriving during quiesce is **paused/retried — never dropped**
  (the DB is the user's only history), proven by a test that fires a write
  into an active quiesce window and asserts it lands after un-quiesce.
  *Depends on: VACUUM wall-time + quiesce spike. Blocks: Compact database
  command.*

- **Compact database command (S5b).** `compact_database` IPC + progress
  events + preflight disk check + dedicated-connection VACUUM (using the
  quiesce guard) + skip-and-report. *Depends on: Ingest quiesce guard +
  retriable ingest boundary, VACUUM wall-time + quiesce spike, and logically
  after Migration v34 drop (so the dead table is gone before the first
  compaction reclaims its space). Blocks: Compact database UI.*

- **Compact database UI.** Button-only affordance (idle-trigger OUT of MVP)
  + progress/skip-report state; wire to the command and events; add
  `ipcFixtures.ts` stubs. *Close condition:* the button exists in the
  existing settings/systems surface, clicking it runs `compact_database`,
  and the progress and skip-report states render from the progress events.
  *Depends on: Compact database command.*

- **Shared `useCachedInvoke` hook.** Extract cache/debounce/dedupe/SWR from
  `useModelAnalytics`. *Independent of backend work. Blocks: Refactor
  useModelAnalytics, all hook ports.*

- **Refactor useModelAnalytics onto shared hook.** Behavior-preserving
  reference port. *Depends on: Shared useCachedInvoke hook. Blocks: nothing,
  but de-risks the ports.*

- **Port Now/Trends/Charts/Context hooks (S1/S2).** Move `useTokenData`,
  `useCodeStats`, `useCodeInsights`, `useLlmRuntimeStats`,
  `useContextSavingsStats`, `useSessionHealth`, `useActivityPattern`,
  `useBreakdownData`, `useAnalyticsData` onto `useCachedInvoke`. *Depends on:
  Refactor useModelAnalytics onto shared hook. Blocks: Dedupe redundant
  invokes.* Can fan out one hook per bead.

- **Dedupe redundant invokes (S4).** Hoist `get_token_hostnames` out of the
  range-keyed fetch; collapse `get_llm_runtime_stats` ×2 and
  `get_snapshot_count` ×2 (Open Q8 resolved: lift a single
  `get_snapshot_count` into `AnalyticsView`, shared to `SnapshotGate` and
  the tab hook via props/context); reuse `useCodeInsights` current-window
  series. *Acceptance:* on a cache-warm current window, the comparison cards
  issue **0 new overlapping history queries** (reuse ≥1 already-fetched
  series). *Depends on: Port Now/Trends/Charts/Context hooks (the shared
  cache is what makes the reuse free).*

- **Follow-up bead: retention/pruning of `tool_actions` + `session_events`
  (Q6 / Spec Review).** File only; out of scope for this feature. *Depends
  on: nothing. No downstream.*

- **Timing measurement pass (Q4).** Capture before/after timing logs on the
  real 7.45 GB DB for a wide-range Models switch and a Now switch; record
  against guidance targets. **Non-gating.** Note plainly: the MVP does **not
  reduce cold-call Models cost** — `get_model_usage_overview`'s temp-table
  materialization is retained pending benchmark, so caching only helps warm
  repeats. *Close action:* if the warm/cold guidance targets are missed,
  **file a follow-up bead** that explicitly reopens the temp-table→CTE
  question (spec Open Q7). *Depends on: Cache model commands, Per-command
  timing logs, Dedupe redundant invokes.*

- **Document feature in lat.md and pass `lat check`.** Update
  `lat.md/backend.md` (cache primitive, migration v34, `compact_database` +
  events), `lat.md/frontend.md` (`useCachedInvoke` + ported hooks), and
  `lat.md/data-flow.md` (quiesce/compaction flow); add test-spec sections
  with `// @lat:` refs per repo convention. *Close criterion:* `lat check`
  is green. *Depends on: all implementation items (Backend cache primitive,
  Cache model commands, Cache S7 endpoints, Migration v34 drop, Ingest
  quiesce guard, Compact database command, Compact database UI, Shared
  useCachedInvoke hook, Refactor useModelAnalytics, Port hooks, Dedupe
  redundant invokes).* Final item.

---

## Alignment fixes applied

- (must) Concrete criteria replace hedges: `useCodeInsights` comparison
  cards issue **0 new overlapping history queries** on a cache-warm current
  window (reuse ≥1 fetched series), and the deferred `get_all_bucket_stats`
  bound is stated as "served from cache on repeat within TTL."
- (must) Frontend per-table invalidation: the shared `useCachedInvoke` layer
  wires an explicit event→hook subscription map (table added to Architecture
  §2); each hook subscribes only to events for the tables its commands read,
  never revalidating unrelated hooks.
- (must) New final Sequencing item "Document feature in lat.md and pass
  `lat check`" depending on all implementation items, updating
  `backend.md`/`frontend.md`/`data-flow.md` with test-spec `// @lat:` refs;
  close = `lat check` green.
- (must) Split out a standalone "Ingest quiesce guard + retriable ingest
  boundary" Sequencing item (guard in lib.rs, retriable HTTP ingest,
  backfill-worker check; write during quiesce paused/retried never dropped,
  proven by test); "Compact database command" now depends on it.
- (must) Backend-cache section specifies the probe-failure path: any probe
  error → cache miss, recompute fresh, log at `warn`, fail-open, never serve
  stale; matching Testing entry "probe raises → miss, no user-facing error."
- (should) Embedded open decisions closed: Compact-database UI is
  button-only (idle-trigger out of MVP) with a concrete close condition;
  SnapshotGate dedupe resolved by lift-to-parent (single `get_snapshot_count`
  in `AnalyticsView`); Now-tab liveness resolved as SWR-everywhere with the
  single TTL constant, no special-case exemption.
- (should) Frontend test-runner decision recorded: hook-port "done" =
  typecheck + lint + manual dev-IPC-mock verification; `vitest` is a separate
  approval-gated follow-up bead, and the automated SWR/race specs move there.
- (should) Cache-probe spike notes `SELECT COUNT(*)` is a full-table scan,
  adds a <5%-of-guarded-query acceptance threshold, and documents the
  `MAX(rowid)`/`MAX(observed_at_ms)`-only fallback (deletes then TTL-only).
- (should) One-way-door documentation folded into the Migration v34 item's
  acceptance: the bead cannot close without both the migration comment and
  the release-note text.
- (should) Timing-measurement pass kept non-gating with a close action to
  file a follow-up bead (reopening temp-table→CTE, Open Q7) if targets are
  missed; plan states plainly the MVP does not reduce cold-call Models cost
  (materialization retained pending benchmark).
- (should) Backend error surfacing stated once in the backend-cache section:
  a cached command that hits a real query error propagates it to the caller
  as today — no silent stale serve.
