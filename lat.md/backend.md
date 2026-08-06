# Backend

The Rust backend handles storage, ingestion, search, LLM analysis, provider lifecycle management, and the cross-platform status indicator.

It communicates with the frontend through a broad Tauri IPC surface and documented push events.

## Entry Point

[[src-tauri/src/lib.rs]] is the application entry point. It initializes storage, starts the HTTP server, registers all Tauri commands, sets up the tray icon, and launches [[architecture#Background Tasks]].

Tauri plugins configured: `tauri-plugin-dialog`, `tauri-plugin-log`, `tauri-plugin-single-instance`, `tauri-plugin-updater`, and `tauri-plugin-window-state`. Session transcript catch-up is no longer part of app launch; the Sessions window requests an incremental sync when search is opened.

[[src-tauri/src/lib.rs#initialize_storage_or_report_fatal]] publishes the process-wide storage handle or returns `None`, and the setup path abandons the rest of startup rather than running against absent storage. It never calls `process::exit` itself: a bare exit made a failed migration look like an app that silently refuses to launch. [[src-tauri/src/lib.rs#report_fatal_storage_failure]] instead hides (not closes — a close on the last window requests app exit and would race the dialog away) every window and queues a `tauri-plugin-dialog` error dialog naming the failure and the database folder, exiting from the dialog callback. The dialog cannot be shown synchronously because setup runs inside the event loop's `Ready` handler and `blocking_show` would freeze the thread it needs. Because termination then hangs off a callback, a session with no working dialog backend would leave a hidden UI-less process alive forever, so a watchdog armed alongside the dialog exits after `FATAL_STORAGE_DIALOG_TIMEOUT` (60s) regardless. Callback and watchdog race for one function-scoped `AtomicBool`, so exactly one terminates and the loser is a no-op.

## HTTP API Server

[[src-tauri/src/server.rs]] (995 lines) runs an Axum HTTP server on port 19876 (configurable via `QUILL_PORT` env var) to receive data from external hook scripts.

### Authentication

All endpoints require a Bearer token validated with constant-time comparison (`subtle` crate). The token is generated on first launch by [[src-tauri/src/auth.rs]] and stored at `~/.local/share/com.quilltoolkit.app/auth_secret` with mode 0o600.

### Rate Limiting

Sliding window rate limiter with 60-second buckets. Limits per endpoint type:

| Category | Limit |
|----------|-------|
| General | 100 req/min |
| Observations | 500 req/min |
| Context savings | 500 req/min |
| Session notify | 500 req/min |
| Session messages | 100 req/min |

`/api/v1/hooks/observed` (feature 009) shares the **Observations** bucket because both endpoints accept hook-fire telemetry whose call rate scales with tool-call volume in active sessions, and a hook chain that fires `PreToolUse` + `PostToolUse` + Quill's own scripts can saturate a stricter limit on a heavy bash-driven turn. The handler runs `check_auth` → `check_rate_limit_with_max(obs_rate_limiter, MAX_OBS_REQUESTS)` → validation ([[src-tauri/src/integrations/codex.rs#is_supported_hook_event|the shared 11-event lifecycle set]], ISO-8601 timestamp parse, length caps on `tool_name`/`hook_matcher`/`agent_id`) → background insert before returning `202 Accepted`, preserving the fast-ack contract observed by `src-tauri/codex-integration/scripts/hook-observe.cjs`. After authentication, any rejection before lifecycle folding invalidates an identifiable exact provider/host/root and emits `hooks-observed-updated`; 429s and malformed agent identities therefore make live coverage unknown instead of preserving a stale count.

### Endpoints

The HTTP API exposes 15 endpoints for token ingestion, context savings, learning observations, session indexing, and hook telemetry across Claude Code and Codex.

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/api/v1/health` | Health check |
| POST | `/api/v1/tokens` | Record token usage from hook scripts |
| POST | `/api/v1/context-savings/events` | Store context savings events from hooks and MCP tools |
| POST | `/api/v1/learning/observations` | Store tool-use observations |
| GET | `/api/v1/learning/observations` | Retrieve unanalyzed observations |
| GET | `/api/v1/learning/status` | Learning system status |
| POST | `/api/v1/learning/runs` | Record a learning analysis run |
| GET | `/api/v1/learning/runs` | Retrieve learning run history |
| POST | `/api/v1/learning/rules` | Store discovered behavioral rules |
| POST | `/api/v1/sessions/notify` | Notify of new session JSONL file |
| POST | `/api/v1/sessions/messages` | Ingest session messages for indexing |
| GET | `/api/v1/sessions/search` | Full-text search sessions |
| GET | `/api/v1/sessions/context` | Get surrounding messages |
| GET | `/api/v1/sessions/facets` | Get search facets |
| POST | `/api/v1/hooks/observed` | Record observed lifecycle-hook fires from Codex (Claude side reads from transcripts) |

Each endpoint validates input (length limits, range checks, type validation) before processing. Provider-aware payloads default legacy callers to `claude`, while new Claude and Codex hooks send explicit provider tags for telemetry and session ingestion. Hook-facing observation and session-ingest POSTs acknowledge after validation and finish SQLite/Tantivy work on background blocking tasks so CLI hooks do not wait on local indexing. Local hook scripts treat receipt of response headers as the success boundary and use a short 1.5-second local timeout, which keeps the CLI path tolerant of slow response teardown without waiting on background indexing.

### Maintenance quiesce

[[src-tauri/src/lib.rs#begin_ingest_quiesce]] gives maintenance exclusive access to SQLite while HTTP token ingest returns retriable `503 Service Unavailable` responses and retained-model backfill waits to persist its next mutation.

The gate is process-wide and reader/writer based: maintenance obtains the writer side before setting its visible quiesce flag, while each guarded ingest or backfill mutation obtains a reader permit. A write already admitted completes before maintenance begins; a write that races after admission waits until the lease is released, so the system never drops history because of a transient maintenance lock. The `write_arriving_during_quiesce_lands_after_unquiesce` regression test proves that a blocked write remains absent during the active window and lands once maintenance ends.

[[src-tauri/src/lib.rs#try_begin_ingest_quiesce]] is the non-blocking variant,
and it exists because `begin_ingest_quiesce` is a bare `RwLock::write()`: a
second caller does not fail, it waits unboundedly. With Compact on one button
and Prune on another in the same settings section, a user who clicks both would
get a frozen second command, no feedback, and a doubled quiesce window. The
retention path therefore acquires through `try_write()` and reports a structured
skip. `MAINTENANCE_IN_PROGRESS` is set only on success, so a refused attempt
cannot make the HTTP surface start returning 503s for a lease it does not hold.
`compact_database` deliberately keeps the blocking acquire — the frontend's
shared busy state, not a backend change, is what stops the two from stacking.

#### Maintenance Quiesce Test Specs

These specs prove maintenance excludes writes without turning a transient
database operation into lost ingest history.

##### Deferred Ingest Is Preserved

An ingest write admitted while maintenance owns the writer gate must stay
pending for the entire quiesce window, then execute after the guard releases.

## Database

[[src-tauri/src/storage.rs]] manages a SQLite database with WAL mode and 5-second busy timeout. The largest backend module.

Writes and maintenance use one mutex-protected primary connection. Widget analytics reads instead open disposable read-only connections with the same timeout, allowing WAL concurrency while each multi-statement response retains one deferred snapshot; [[backend#Backend#Database#View Query Reader Connections]] inventories the migrated commands.

### Location

The SQLite database file path varies by operating system.

- Linux: `~/.local/share/com.quilltoolkit.app/usage.db`
- macOS: `~/Library/Application Support/com.quilltoolkit.app/usage.db`

### VACUUM maintenance evidence

The frozen measurement in `specs/013-analytics-query-perf/vacuum-spike.md` records compaction cost and the ingest-quiesce behavior that production maintenance preserves.

### Provider/source index plan evidence

The frozen proof in `specs/014-retention-pruning/eqp-index-drop-proof.md` established that SQLite uses the partial source-owned unique index after a redundant plain provider/source index is removed.

Current regression tests extend that proof across all five transcript analytics tables and the bundled SQLite version. Each source-owned delete must report its `uidx_*_owned` seek and no table scan.
### Redundant provider/source index drop

[[src-tauri/src/storage.rs#ensure_startup_indexes]] drops `idx_session_events_provider_source(provider, source_key)` on every open, because the partial `uidx_se_owned` already serves every statement that index existed for.

The drop lives in a function whose name says "ensure" rather than in a
migration, and that is deliberate: `ensure_startup_indexes` is idempotent, runs
on every open, and never created this index — only migration 30 did, and only
for databases below v30. Putting the drop there needs no schema-version bump
and so cannot cause a `SCHEMA_TOO_NEW` lockout; an older build that reopens the
file simply finds one fewer index. Because the function's name now describes
only half of what it does, the second responsibility is written down on the
function itself.

Correctness rests on `source_key = ?` implying `source_key IS NOT NULL`, which
keeps the partial `uidx_se_owned(provider, source_key, event_key) WHERE
source_key IS NOT NULL` usable for all three source-owned delete sites. That
implication is SQLite-version-dependent, so it was proved by the
[[backend#Backend#Database#Provider/source index plan evidence]] before the drop
shipped and is pinned permanently by the test specs below. Retention drops
exactly this one index and no other — `idx_se_timestamp_chain` is recreated in
the same function precisely because `get_llm_runtime_stats` pins it with
`INDEXED BY`.

#### Provider Source Index Drop Test Specs

These specs keep the drop honest in both directions: the redundant index must
really be gone, and nothing else may go with it or regress to a scan.

##### Redundant Index Gone After Open

A fresh open must leave no `idx_session_events_provider_source` in
`sqlite_master`, while `idx_se_timestamp_chain` survives and the query pinning
it with `INDEXED BY` still prepares.

Reopening the already-dropped database must then be a silent no-op rather than
an error, which is what makes the drop safe to run on every open.

##### Owned Deletes Keep Their Index Seek

All three `(provider, source_key)` `session_events` delete statements must
still report a search through `uidx_se_owned` and never a `SCAN`, so a future
query change cannot silently trade the drop for a full table scan.

### Provider/source index footprint evidence

The frozen measurement in `specs/014-retention-pruning/index-drop-measurement.md` records the storage benefit and maintenance cost of removing a redundant provider/source index.

On the measured 7.55 GB database, the drop took 416 ms, dirtied 471 KiB of WAL, and reclaimed 727 MB after VACUUM. Production keeps the proven index removal; the one-off corpus utility is no longer shipped.
### Retention timing spike

The frozen measurement in `specs/014-retention-pruning/retention-timing-spike.md` supplies the numeric budgets used by the retention delete engine and preflight.

The measurement used the same fixture retained by acceptance tests. Against a
2M-row, 1.29 GB database it swept five chunk sizes and fixed: a
**25,000-row chunk** (the largest whose p95 transaction hold stays under one
second — the visible-progress threshold for a background job, not the
instantaneous-response one), **789 WAL bytes per row**, **11 TEMP bytes per
doomed row**, a **3-chunk** free-space re-check interval, and Counting-phase,
stale-preview and total wall-time budgets at three times the measured value.

Three results shape the engine beyond the constants. `PRAGMA temp_store` is
pinned to `MEMORY`: both settings build the same 7.4 MB doomed-rowid b-tree at
identical wall time, so the only question is whether those bytes land in RSS
or on a temp filesystem the disk preflight may not have measured.
`PRAGMA wal_checkpoint(TRUNCATE)` after each chunk holds post-checkpoint WAL at
**zero bytes** at every chunk size, so WAL really is bounded by one chunk
rather than by the run. And the Counting phase is **6.4%** of the run, which
settles the design question the plan reserved for this spike: the second scan
is cheap enough that `run_retention_maintenance` keeps rescanning under its own
lease instead of `preview_retention` holding the lease across the user's
confirmation dialog.

Two incidental findings are recorded rather than acted on: `tool_actions` does
not table-scan for the doomed set — the planner walks the partial unique index
`uidx_ta_owned` — and `idx_se_timestamp` makes the `session_events` scan 2.37×
*slower* than the `idx_se_timestamp_chain` plan SQLite falls back to without
it, measured against a same-corpus control that cancels page-cache bias.
Full numbers live in `specs/014-retention-pruning/retention-timing-spike.md`.

### Widget query benchmark corpus

Feature 020's frozen evidence records production-scale query timings, planner audits, and corrected rollup volume without keeping corpus tooling in the application crate.

The immutable protocols and results remain in `specs/020-widget-query-perf/timing-measurement.md`. They established the current read-only connection settings, bounded planner analysis, raw/hybrid overview parity, and a conservative 1.856 GB annual model-rollup envelope.
### Database compaction

[[src-tauri/src/lib.rs#compact_database]] exposes user-triggered SQLite compaction with observable progress and a structured, safe skip result.

The command acquires [[src-tauri/src/lib.rs#begin_ingest_quiesce]] before it
checks free disk space and runs [[src-tauri/src/storage.rs#Storage#vacuum_database]]
on a dedicated SQLite connection. Its result reports the before/after database
footprint on success, or a reason with unchanged size when preflight or VACUUM
cannot proceed. It emits `compact-database-progress` for the disk-space and
VACUUM phases, then emits `compact-database-finished` with the structured result
after all successful maintenance and the ingest-quiesce lease are complete.

#### Bounded Query Planner Analysis

Successful manual compaction refreshes SQLite planner statistics without adding startup work or changing retention semantics.

After VACUUM, [[src-tauri/src/storage.rs#Storage#run_bounded_database_analysis]]
opens a dedicated writer, applies `PRAGMA analysis_limit=1000`, runs `ANALYZE`
inside one immediate transaction, requires `sqlite_stat1`, refreshes the
long-lived writer's planner state, then performs a TRUNCATE checkpoint before
the compaction lease can release. Any SQL,
verification, or checkpoint fault propagates with its operation named.

The long-lived writer reloads `sqlite_stat1` and flushes cached prepared
statements before the final checkpoint. View readers are already disposable
and load the new statistics on open. Result caches remain valid because planner
statistics cannot change query semantics. Skipped preflight/VACUUM paths and
retention-triggered compaction do not run ANALYZE.

Frozen planner and timing evidence remains in `specs/020-widget-query-perf/`. Production keeps the verified bounded ANALYZE path without shipping its one-off trace and corpus audit machinery.
#### Database Compaction Test Specs

These specs pin the user-triggered maintenance result contract without needing
an application window or a production-sized database.

##### Completed Footprint Report

A successful preflight followed by VACUUM must report `completed`, preserve an
empty skip reason, and return the actual before/after database footprint.

### Retention pruning

Feature 014's opt-in age window: the user picks a preset, previews, confirms, and Quill deletes source-owned rows older than the cutoff from `tool_actions` and `session_events` only, then compacts. Nothing is scheduled.

This section is the map. Each part is documented where it lives — the durable
grammar in [[backend#Backend#Database#Retention policy primitive]], the
destructive core in [[backend#Backend#Database#Retention delete engine]], the
durability half in [[backend#Backend#Database#Insert-time watermark filtering]],
the push surface in [[backend#Backend#Database#Retention maintenance events]],
the completion step in
[[backend#Backend#Database#Analytics cache invalidation on prune]], the command
boundary in [[backend#Backend#Tauri IPC Commands#Retention policy commands]],
[[backend#Backend#Tauri IPC Commands#Retention preview command]] and
[[backend#Backend#Tauri IPC Commands#Composite retention command]], and the
reader-side honesty treatment in
[[frontend#Frontend#Components#Retention Degradation]].

**The data model is three rows of `settings`, not a table.**
`retention.window_days` (a preset from `{30, 90, 180, 365}`),
`retention.watermark` (a conforming 24-character `Z` timestamp) and
`retention.last_run` (the JSON audit record) are independently absent-able, and
absent on all three is the default state of every database that exists today.
There is no migration and no schema-version bump, which is exactly what makes an
older build's reopen a non-event, and it is why disabling retention *deletes*
`retention.window_days` instead of writing a sentinel — one "never" state, not
two. The 30-day floor is a guarantee rather than a suggestion: `range_to_duration`
caps every range-based reader at 30 days, so a shorter window would silently
starve `get_code_stats`, `get_code_stats_history` and `get_llm_runtime_stats`.

**The watermark is the whole durability argument, and its timing is part of it.**
It only ever moves forward — `max(existing, cutoff)`, applied with the read and
the write inside one transaction — and it reaches the run's cutoff **before the
first chunk transaction opens**, never at the end of the run and never after
VACUUM. Rows deleted at a stricter cutoff must stay deleted even if the process
dies mid-run, so a committed chunk leaves the watermark permanently advanced
through a `"partial"` outcome, a skipped VACUUM or a failed one. The mirror rule
is that a run which skips at the delete-phase preflight must **not** advance it:
nothing was deleted, so suppressing future inserts would take away history the
user never consented to lose.

**Exactly one index is dropped, and it is not a retention index.** Phase 1
removes `idx_session_events_provider_source` and nothing else — see
[[backend#Backend#Database#Redundant provider/source index drop]].
`idx_se_timestamp_chain` is recreated in the same startup function because
`get_llm_runtime_stats` pins it with `INDEXED BY`, and no `idx_se_timestamp` is
added for retention's benefit: the
[[backend#Backend#Database#Retention timing spike]] measured that index making
the `session_events` scan 2.37× slower than the plan SQLite picks without it.

**Three statuses, and `"partial"` is a real one.** A run reports `completed`,
`skipped` or `partial`. `partial` exists rather than `completed` plus an
`interrupted` flag because its whole job is to say what went wrong, so the audit
record refuses to validate a `partial` with no `error_reason`. Compaction
reports separately: a run that removed rows but could not VACUUM is a completed
prune with a skipped compaction, which is the legitimate "rows are gone, bytes
are not back yet" outcome the UI has to state rather than hide.

**Four commands and two events.** `get_retention_policy` and
`set_retention_policy` read and write the window; `preview_retention` counts and
mints the cutoff the user consents to; `run_retention_maintenance` takes that
confirmed cutoff, deletes, compacts, and invalidates the analytics caches.
`retention-maintenance-progress` and `retention-maintenance-finished` are the
only two events, shared by preview and run so the frontend keeps one listener
pair, and no third event enters the IPC surface for cache invalidation — that
reuses `transcript-analytics-updated`.

**Retention has no learning stakeholder, and here is when that expires.**
[[src-tauri/src/learning.rs#analyze_sessions_stream]] builds its session digests
by reading transcript JSONL through
[[src-tauri/src/sessions.rs#extract_messages_from_jsonl]] and never touches
`tool_actions`, so pruning cannot starve rule learning. That is a fact about
today's code, not a design guarantee: **a future learning pipeline that sources
observations from `tool_actions` becomes a retention stakeholder, and this entire
analysis must be redone at that point.**

### Retention fixture

[[src-tauri/src/retention_fixture.rs#build_retention_fixture]] builds the test-only frozen synthetic corpus used by every retention acceptance test.

The builder sets `QUILL_DEMO_MODE` and `QUILL_DATA_DIR` to a fresh temp
directory and creates the database through `Storage::init`, so the schema comes
from real migrations and startup index cleanup. That env block is
process-global, so **every consuming test must be annotated `#[serial]`**. The
overrides remain set afterwards, matching the `init_storage_in` harness, so a
consumer's own `Storage::init()` lands on the same database.

Rows sit in fixed 30-day buckets counted back from an anchor instant, bucket 0
being the most recent. Each bucket carries, per table, a known number of
source-owned conforming rows, a known number of live `source_key IS NULL` rows,
and — for the two retention target tables only — one row per
[[src-tauri/src/retention_fixture.rs#NonConformingShape]]: a `+00:00` offset, a
seconds-precision `Z`, and a 24-character `+0000` form. The three shapes fail
the `length(timestamp) = 24 AND timestamp LIKE '%Z'` guard in different halves,
so a guard implemented with only one half still fails a test. Every row instant
is derived arithmetically from its bucket, table and population, which keeps
per-month counts exact and guarantees no row ever lands on a cutoff produced by
[[src-tauri/src/retention_fixture.rs#RetentionFixturePlan#boundary]]. Consumers
assert against [[src-tauri/src/retention_fixture.rs#RetentionFixturePlan]]
rather than hand-copied literals, so "the run deleted exactly the pre-cutoff
rows and no others" is an exact equality.

#### Retention Fixture Test Specs

These specs pin the corpus contract the whole retention epic depends on: exact
counts, a clean cutoff split, and a schema that really came from `Storage::init`.

##### Exact Per-Month Counts

Every table and population must hold exactly the number of rows the plan
declares, and the three sibling tables must hold no planted non-conformance, so
a later run can be proved to have left them alone without a caveat.

##### Boundary Row Split

For every retained-month count from zero through the full corpus, the owned
conforming rows older than that boundary must equal the plan's own figure —
this is the equality every delete-engine acceptance assertion is built on.

##### Guard Straddling Rows

Live rows must exist on both sides of a cutoff and non-conforming rows must be
present in both target tables, so a filter that leaked into the live path or a
guard missing one half cannot pass unnoticed.

##### Migrated Schema And Reopen

The fixture database must carry both migration-30 indexes and the startup-only
`idx_se_timestamp_chain`, and a fresh `Storage::init()` must reopen the same
database, proving the env-override contract rather than assuming it.

##### Spec Validation

A zero-valued spec field and a per-month row count that would push rows outside
their own bucket must both be rejected with their specific error variants, not
silently produce a corpus whose boundary math no longer holds.

### Retention policy primitive

[[src-tauri/src/retention.rs]] owns everything durable about retention: three rows of the existing `settings` table, their value grammars, the two serialized shapes, cutoff derivation, and the monotonic watermark rule. There is no schema migration and no schema-version bump.

The keys follow the backend-only dotted convention used by other settings.
`retention.window_days` holds the configured window as a
decimal integer; `retention.watermark` holds the insert-time cutoff as a
conforming 24-character timestamp; `retention.last_run` holds the JSON
[[src-tauri/src/retention.rs#RetentionAuditRecord]]. Every row is
independently absent-able, and absent on all three is the default state of every
existing and new database — which is why disabling retention *deletes*
`retention.window_days` rather than writing a literal: there is one "never"
state, not two. Storing these in `settings` rather than in a new table is what
makes an older build's reopen a non-event: it reads the table normally, does not
know the keys, and opens without complaint.

[[src-tauri/src/retention.rs#derive_retention_cutoff]] renders
`now - window_days` as a 24-character millisecond-precision `Z` timestamp, so the
scan, the deletes and the insert filter can all use a plain byte comparison
against stored timestamps. It re-validates the window against
[[src-tauri/src/retention.rs#RETENTION_WINDOW_PRESETS]] because the 30-day floor
is the guarantee that `get_code_stats`, `get_code_stats_history` and
`get_llm_runtime_stats` — all capped at 30 days by `range_to_duration` — can
never ask for pruned data. A window that bypassed that floor at any boundary
would silently revoke the guarantee, so it is rejected on write, on read, and
again here.

[[src-tauri/src/retention.rs#advanced_watermark]] is `max(existing, cutoff)` and
returns a new value rather than mutating one.
[[src-tauri/src/storage.rs#Storage#advance_retention_watermark]] applies it with
the read and the write in a single transaction, so two callers cannot interleave
into a retreat. Monotonicity is the whole durability argument: rows deleted at a
stricter cutoff must stay deleted, so narrowing the configured window later — or
clearing it to never — must never let the watermark move back and resurrect them
on the next reparse.

Reads are tolerant because these rows survive downgrades, hand edits and
interrupted writes, and none of them may be able to block a run. A
`retention.window_days` outside the preset set parses as "never" rather than as
itself; a non-conforming `retention.watermark` is treated as absent so a value
SQLite cannot order never becomes a filter; an unparseable or unknown-schema
`retention.last_run` parses as `None`. All three log at `warn` and none returns
an error. Writes are strict by contrast:
[[src-tauri/src/retention.rs#RetentionAuditRecord#validate]] refuses a
`"partial"` record with no `error_reason`, because the only reason `partial`
exists as a third status — rather than `completed` plus an `interrupted` flag —
is that it says what went wrong.

#### Retention Policy Primitive Test Specs

These specs pin the invariants at the layer that owns them, so a monotonicity or
parsing bug fails here instead of surfacing as a baffling end-to-end failure.

##### Watermark Monotonicity

Advancing to a 90-day cutoff and then calling the advance helper with an earlier
365-day cutoff must not retreat the watermark, and clearing the window to never
must leave it in place — the two ways a resurrection bug would enter.

##### Audit Record Round Trip

A record written through the helper must read back through `get_retention_policy`
with its cutoff, timestamp, status, error reason and per-table counts intact, and
must survive a reopen.

The record's entire purpose is to answer "what happened" long after the toast is
gone, so a field that does not survive the round trip is a field that is not
really there.

##### Corrupted Audit Value

An unparseable `retention.last_run` must read as absent and must not block a
subsequent write, so a truncated or hand-edited value degrades the audit trail
rather than wedging retention.

### Analytics cache invalidation on prune

[[src-tauri/src/lib.rs#invalidate_analytics_after_retention]] is the single step every retention run ends on: it drains all five in-process analytics caches and emits `transcript-analytics-updated` so no reader keeps serving pre-prune counts.

The in-process half exists because a prune is invisible to the cache's own
freshness check. [[backend#Backend#Database#Schema#Model Analytics Evidence#Analytics Cache Primitive]]
validates an entry against max-only high-water markers, and a DELETE never
advances one, so a payload built before the prune still matches the post-prune
database and the 45-second TTL is the only thing that would retire it.
[[src-tauri/src/storage.rs#Storage#clear_analytics_caches]] therefore drops
every entry unconditionally rather than reasoning about which command could
have been affected. Neither `tool_actions` nor `session_events` is a
`CacheTable` today, so the blast radius is currently zero — that is exactly why
the drain is cheap, and why it is written now instead of being left as a trap
for whichever command gets cached next.

The frontend half reuses the event `useCodeStats`, `useBreakdownData`,
`useLlmRuntimeStats` and `useCodeInsights` already listen for, so a prune
revalidates those hooks through the same channel an ingest does and no new
push event enters the IPC surface.

The emitter is a closure rather than a `tauri::AppHandle` so the contract is
provable without an application window;
[[src-tauri/src/lib.rs#emit_retention_analytics_invalidation]] is the closure
the composite command passes. Nothing here returns an error: invalidation runs
after the run's outcome is already decided, and a failed notification must
never turn a completed prune into a failed one.

#### Analytics Cache Invalidation Test Specs

This spec pins the contract at the layer that owns it, so a missed cache or a
missing emission fails here rather than as an unexplained stale count in the UI.

##### All Caches Drained And Event Emitted

With all three caches warmed through their real read paths, the completion path
must leave every one of them empty and must emit exactly
`transcript-analytics-updated`.

A drain that misses one cache and a drain that forgets to notify are the same
defect to a user — a stale count — so both halves are asserted in one test.
Warming through the real commands rather than by hand-inserting entries is what
makes the emptiness assertion mean something.

### Retention maintenance events

[[src-tauri/src/lib.rs#emit_retention_maintenance_progress]] and [[src-tauri/src/lib.rs#emit_retention_maintenance_finished]] are the only writers of the two retention events, so the event names and the phase vocabulary have one definition.

The pair mirrors database compaction deliberately:
[[src-tauri/src/lib.rs#RetentionMaintenanceProgress]] has the same
`{ phase, pct }` shape as `DatabaseCompactionProgress`, so the Settings UI
renders both maintenance paths with one component. The scaffolding ships ahead
of the commands that emit through it because *two* commands do — preview reuses
[[src-tauri/src/lib.rs#RETENTION_MAINTENANCE_PROGRESS_EVENT]] for its counting
phase rather than owning a third event, which is what lets the frontend keep a
single listener pair for "previewing" and "running".

The phase vocabulary is a fixed set — `Counting rows`, `Checking disk space`,
`Removing old rows`, `Compacting database` — and `phase` is typed `&'static str`
so a caller passes a member of that set instead of an ad-hoc string; the phases
a user can observe stay enumerable from one place. Counting is a single
`CREATE TEMP TABLE … AS SELECT` with no natural progress signal, so its `pct`
comes from a wall-clock heartbeat instead of sitting at zero; the delete phase
advances per chunk so a several-hundred-thousand-row run visibly moves.

[[src-tauri/src/lib.rs#emit_retention_maintenance_finished]] is generic over its
payload so the event name could land before the preview and maintenance result
types exist. Both emitters log a failed emit and return: the run has already
happened and its outcome is durable in `retention.last_run`, so a dropped event
must not turn into a failed maintenance run.

### Insert-time watermark filtering

[[src-tauri/src/storage.rs#Storage#replace_transcript_analytics_snapshot]] reads `retention.watermark` inside its own transaction and filters the `session_events` and `tool_actions` insert loops against it. This — not the delete — is what makes a retention run durable.

Resurrection is a normal-path behaviour, not an edge case. The replacement
unconditionally deletes and reinserts a source's whole parse whenever its
mtime, size or content hash changes, so one `--resume` append to a months-old
transcript would restore that source's entire pre-cutoff history, and the
`transcript_analytics_reingest_pending` marker would do it for every retained
source at once. Reading the watermark beside the generation key costs one more
primary-key lookup on a transaction that already reads a `settings` row, and it
turns reconciliation from a threat into an ally: because the replace already
deleted the source's rows, a filtered reinsert also prunes that source's stale
pre-cutoff rows as a side effect.

[[src-tauri/src/retention.rs#retention_insert_verdict]] is the delete guard
inverted in effect: a row is suppressed exactly when
`length(timestamp) = 24 AND timestamp LIKE '%Z' AND timestamp < watermark`
holds — the predicate the delete phase uses. A non-conforming timestamp is
therefore **always inserted**, never suppressed, because it is also never
deleted. The two guards must agree or a row could be suppressed on reinsert
while its original was retained, which is silent data loss with no delete to
account for it. The three outcomes are an enum rather than a boolean so the
pass-through case cannot be collapsed into "not suppressed" by accident.

Scope is deliberately narrow. `response_times`, `skill_usages` and
`hook_invocations` are inserted unfiltered and keep full history — they are not
retention targets, and the "All time" Breakdown toggles read two of them. The
`transcript_analytics_sources` registry row is written exactly as an unfiltered
replacement writes it, so the source stays registered and reconcilable; a
per-source `retained_through` column is ruled out because
`prune_transcript_analytics_sources_for_root` deletes the registry row it would
live on. Live rows never reach this path at all: every other insert site
(`store_live_session_analytics`, `ingest_session_events`, and
`server.rs::persist_remote_session_analytics` through the first of them)
hard-codes `source_key NULL`, and live rows are excluded from retention.

[[src-tauri/src/storage.rs#RetentionInsertFilterCounts]] rides back on
`TranscriptAnalyticsReplacement::Replaced` and keeps the two figures separate,
because they mean opposite things: suppressed rows are retention working, while
non-conforming pass-throughs are rows retention cannot act on in either
direction. `commit_transcript_snapshot` logs both whenever either is non-zero,
so "suppressed 412, passed 3 non-conforming" appears in the reconciliation log
rather than being inferred from a row count that changed.

#### Insert-Time Watermark Test Specs

These specs pin the durability argument end to end: the filter's blast radius,
its conformance guard, and the two paths through which a pruned row could come
back.

##### Watermark Filters Snapshot Inserts

Under an active watermark, only post-cutoff `session_events` and `tool_actions`
rows may land, while every sibling-table row lands unfiltered and the registry
row is written unchanged.

The siblings are `response_times`, `skill_usages` and `hook_invocations`; the
registry assertion covers its fingerprint, status and generation.

This is the blast-radius spec: a filter that reached one table too far would
silently delete history retention never promised to touch, and one that skipped
the registry row would strand the source outside reconciliation.

##### Conformance Guard Pass-Through

Non-conforming pre-cutoff rows must all land, only conforming pre-cutoff rows may
be suppressed, and the replacement summary must report the suppressed and
non-conforming counts separately.

The three planted shapes fail the guard in different halves — a `+00:00` offset,
a seconds-precision `Z`, and a 24-character `+0000` form — so a guard written
with only one half still fails here.

##### Changed Source Does Not Resurrect

After a prune, changing a source's fingerprint and re-driving reconciliation must
not return the pruned rows, and the post-cutoff rows must be present exactly
once.

This is the normal path — an appended transcript — and the reason the watermark
exists rather than a one-off delete.

##### Forced Reparse Does Not Resurrect

Setting `transcript_analytics_reingest_pending` and running startup
reconciliation must produce the same result, because that marker bypasses every
freshness short-circuit and reparses each retained source in full.

It is the widest resurrection window in the system, so it gets its own spec
rather than being assumed to follow from the changed-source case.

##### Live Rows Ignore The Watermark

`store_live_session_analytics` and `ingest_session_events` must insert their
`source_key IS NULL` rows on both sides of the cutoff, and every one must land.

Live rows are outside retention's scope by design, so a watermark leaking into
either live path would delete data the user was never asked about.

### tool_detail payload carve-out

The same `tool_actions` insert loop binds NULL for `full_input` and
`full_output` whenever the row's category is
[[src-tauri/src/storage.rs#TOOL_DETAIL_CATEGORY]]. This is retention's second
footprint lever: bytes no reader ever asks for, dropped at the bind.

The grep behind it is closed. The only SQL readers of either column are
`get_code_stats`, `get_code_stats_history` and `get_batch_session_code_stats`,
and all three are gated on `category = 'code_change'`; the Tantivy
`tool_details` field is built from `action.summary`, not from these columns; the
MCP server never selects from `tool_actions` at all; and no frontend file
references either name. A `tool_detail` row therefore carries up to 10KB of
`full_input` plus 10KB of `full_output` that nothing in the product reads back.

The row itself stays. `get_session_breakdown` and `get_session_subagent_tree`
scan and `COUNT(*)` `tool_actions` with **no** category filter, so dropping
`tool_detail` rows would deflate per-agent tool-call counts and could erase a
sub-agent from the tree entirely whenever a `tool_detail` action is its only
`agent_id`-bearing row. Omitting the two columns keeps every row-shaped
invariant — row count, `action_key` uniqueness, `agent_id`, `message_id`,
timestamps — and costs only the dead bytes.

Scope is the SQL bind and nothing else. `ToolAction.full_input` must keep being
populated by the parser, because
`sessions.rs::extract_skill_accesses_from_tool_action` reads it **in memory**
for `Read`, `exec_command` and `Skill` — all three of which classify as
`tool_detail` — while building `skill_usages`, a table retention never prunes.
That reader consumes the parsed action, never the table, so it is unaffected by
what the bind writes. `code_change` keeps its payloads because the code-stats
queries re-parse them for legacy rows, and `command` — the largest bucket — is
deliberately left alone; this decision does not widen to it.

The policy is forward-only. Existing rows keep their payloads and are never
retroactively NULLed: `full_input IS NOT NULL` is load-bearing in all three
code-stats queries, which read the column as a non-optional string.

#### Tool Detail Payload Test Specs

This spec pins the carve-out's two halves, since neither is visible in a row
count: the payloads that must vanish and the row that must not.

##### Tool Detail Rows Land Without Payloads

Given one `tool_detail`, one `code_change` and one `command` row that all carry
payloads, the `tool_detail` row must read back present but NULL in both payload
columns while the other two keep their values.

The row's presence is asserted alongside the NULLs because dropping the row
would also satisfy a NULL-only check, and the category-agnostic subagent
readers depend on it existing.

### Retention delete engine

[[src-tauri/src/retention_engine.rs#run_retention_delete_phase]] runs retention's bounded, audit-backed destructive phase.

It uses a dedicated connection, one-pass scan, optional pre-delete JSONL
archive, disk preflight, and chunked delete with a monotonic watermark. Model
observations use normalized integer timestamps against the confirmed cutoff.

It owns no policy — the grammars, the cutoff and the monotonic rule live in the
[[backend#Backend#Database#Retention policy primitive]] — and no UI. Every
numeric constant it uses comes from the
[[backend#Backend#Database#Retention timing spike]]; none was chosen here.

#### Why a dedicated connection

[[src-tauri/src/retention_engine.rs#open_maintenance_connection]] opens the database itself rather than borrowing `Storage`'s primary connection, and pins `PRAGMA temp_store = MEMORY`.

The primary connection is a single process-wide mutex, so a scan-and-delete on
it would block every read IPC — the always-on-top widget included — for the whole
run. On its own WAL connection readers keep reading, and only writes are gated,
by the quiesce lease the caller already holds. `temp_store` is pinned rather than
inherited because the spike measured the same doomed-rowid b-tree under both
settings at identical wall time: the only question is whether those bytes land in
RSS or on a temp filesystem the disk preflight may never have measured, and under
10 MB of RSS is the cheaper answer for a desktop app.

The connection owns both `retention_doomed_*` `TEMP TABLE`s and is dropped before
[[src-tauri/src/retention_engine.rs#run_retention_delete_phase]] returns, because
`vacuum_database` rebuilds the whole file on its own connection and does not
tolerate another one holding schema-visible temp state.

#### The doomed-rowid scan

[[src-tauri/src/retention_engine.rs#scan_doomed_rows]] materializes all three target tables' doomed rowids in one pass each, counts the rows the conformance guard kept, and drives a wall-clock heartbeat while it runs.

`tool_actions` has no index leading with `timestamp`, so a per-chunk
`WHERE timestamp < ?` would rescan the table on every chunk. The single pass
yields the exact preview count for free, makes every chunk a rowid seek, and
freezes the delete set so the result reported is the set that was previewed.

Its two guards are both load-bearing. `source_key IS NOT NULL` excludes live rows,
which no retention path may ever touch. `length(timestamp) = 24 AND timestamp
LIKE '%Z'` excludes timestamps that are not byte-comparable — a `+00:00` form's
`+` sorts before `.` and would mis-compare at the boundary. That second guard is
the delete side of a symmetry with the insert-time filter: a row this scan
refuses to delete is a row the filter must refuse to suppress, or a row could be
dropped on reinsert with no delete to account for it. Guard-failing pre-cutoff
rows are counted and reported as `skipped_nonconforming` instead, a *report*
computed by a byte comparison the guard has already declared unreliable — no row
is ever removed on its basis.

The scan is one opaque `CREATE TEMP TABLE … AS SELECT` that emits no rows and
runs for most of a second on a production corpus, so
[[src-tauri/src/retention_engine.rs#install_scan_heartbeat]] registers a rusqlite
`progress_handler` and nudges the percentage on wall time. It never returns
`true` — that would abort the scan — and never climbs past a ceiling short of
100, because a liveness signal that claims completion it cannot observe is worse
than none. The handler is uninstalled before the delete phase, which has real
per-chunk progress. This is the reason the crate enables rusqlite's `hooks`
feature.

#### Archive before deletion

[[src-tauri/src/retention_engine.rs#write_retention_archive]] streams a complete JSONL sidecar under the same quiesce lease and maintenance connection, then atomically publishes it before any delete transaction opens.

The manifest records schema version, cutoff, window, deletion-candidate counts
and non-conforming counts for all three target tables. Each following line
names its source table, classification and full SQLite row, including `rowid`,
so the sidecar preserves every field instead of projecting an analytics view.
Transcript rows use the preview's source-owned byte-order partition, including
non-conforming rows it reports but keeps. Model observations use the preview's
normalized `observed_at_ms` predicate and are deletion candidates.

The writer checks its per-table totals against the scan before publishing,
flushes and syncs a private temporary file, and uses a no-clobber atomic persist
under `retention-archives/` beside `usage.db`. Any write, serialization, count,
sync or publish failure becomes a structured skip with no watermark advance and
no deleted row. The delete preflight runs after a successful archive so the
sidecar's disk consumption is included in the free-space decision; a refusal
keeps the completed archive and reports its path.

#### The chunk boundary is a value

[[src-tauri/src/retention_engine.rs#drain_target]] materializes one `:max` rowid per chunk transaction and drives both the target delete and the bookkeeping delete from it, then checkpoints the WAL after the commit.

Two independent unordered `SELECT … LIMIT` scans over the same temp table carry
no guarantee of agreeing on row order, so a single scalar boundary is what makes
divergence impossible: no doomed rowid leaves the temp table without its row
being deleted, and no row is deleted that the temp table still claims. The bound
also keeps each `DELETE` a bounded rowid range rather than a full `IN`-subquery
materialization. `DELETE … LIMIT` is not an option at all —
`SQLITE_ENABLE_UPDATE_DELETE_LIMIT` is not compiled into the vendored build.

Each chunk is its own transaction, so an interrupted run leaves committed chunks
committed and no partial row written; the next run simply recomputes its doomed
set. Deleting one row rewrites its entry in every surviving index, so WAL churn
is index-dominated — `PRAGMA wal_checkpoint(TRUNCATE)` after each commit bounds
WAL by one chunk rather than by the run, measured at zero post-checkpoint bytes
at every swept chunk size.

Before the watermark moves, the engine verifies that every affected model group
and finalized runtime source equals its hourly refold. Missing or divergent
coverage refuses the whole prune. Before each chunk deletes detail rows,
[[src-tauri/src/retention_engine.rs#drain_target]] promotes covered hourly rows
to `raw_pruned=1`, clamps the runtime bookmark, and upserts transcript daily
counters into [[backend#Database#Schema#Retention aggregates]] inside the same
SQLite transaction. A committed prune therefore leaves hourly authority plus a
compact session/code-stat view; runtime reads consume only the hourly side, so
daily event counters cannot double-count runtime. A failed chunk leaves raw,
rollups, bookmark, and daily aggregate unchanged. Live rows and a partial or
unrealized trailing runtime turn remain outside the doomed set; a fully doomed
turn is sealed into `runtime_hourly` first.

#### Preflight, and what it is not

[[src-tauri/src/retention_engine.rs#preflight_delete_phase]] requires free disk for one chunk's WAL plus both doomed-rowid temp tables, doubled. Failing it is a skip with a reason, not an error, and it removes no rows and leaves the watermark alone.

This is emphatically **not** the VACUUM preflight and does not subsume it: VACUUM
needs twice the whole file, while the delete phase needs tens of megabytes. A
database can comfortably pass this and fail that, which is the legitimate "rows
removed, bytes not yet reclaimed" outcome the composite command reports as a
completed run with a skipped compaction.
[[src-tauri/src/retention_engine.rs#RetentionDeleteBudget#estimate]] keeps the
TEMP term in the *disk* requirement even though `temp_store` is `MEMORY` and
those bytes are really RSS: at 11.05 B per doomed row the term is a rounding
error beside the WAL term, and carrying it keeps the requirement correct if the
pinned `temp_store` is ever revisited.

A preflight that passed at chunk 0 says nothing about chunk 400, so the loop
re-checks free space every [[src-tauri/src/retention_engine.rs#RETENTION_FREE_SPACE_RECHECK_CHUNKS]]
chunks — a 3.16 µs `statvfs` against a mean chunk hold of 417.7 ms. On failure
the run does not panic and does not continue: it stops at the last committed
chunk and reports `Partial` with the reason.

#### Watermark advance and the audit write

The watermark reaches the run's cutoff **before the first chunk transaction opens**, and the audit record is written on the completed, partial *and* skipped paths.

Advancing it there rather than at the end of the run is what makes an interrupted
destructive run durable: the rows are gone, and a later reparse must not restore
them. It cannot move *inside* the chunk transaction, because the watermark rides
[[src-tauri/src/storage.rs#Storage#advance_retention_watermark]] on the primary
connection, WAL permits exactly one writer, and a primary-connection write issued
while the maintenance connection holds an `IMMEDIATE` transaction would deadlock
the run against itself until `busy_timeout` expired. Advancing just before the
commit rather than just after also closes the only window in which rows could be
gone while the watermark still permitted their reinsertion.

The mirror rule is that a run which **skips at the preflight** must not advance
it: nothing was deleted, so advancing would suppress inserts the user never
consented to lose. Once a chunk commits the watermark stays where it is
regardless of what happens next — a partial run, a skipped VACUUM, a failed one.

The audit record's `bytes_after` equals `bytes_before` here, which is not a
placeholder but the truth: deletes free no filesystem bytes, and only the VACUUM
that may follow changes the file's size. Writing it on the error path too is the
point — a run the user cannot account for afterwards is the failure the record
exists to prevent, and the record must survive a process that never reaches
compaction.

#### Retention Delete Engine Test Specs

These specs pin the destructive invariants: exact deletions, a database that is
consistent at every interruption point, and a watermark that moves exactly when
it should and never when it should not.

##### Chunk Correctness And Idempotency

A chunk size smaller than the doomed set must delete exactly the planned rows and
no others, and drain each `retention_doomed_*` table in lockstep with its target
under the shared `:max` bound.

A row whose timestamp equals the cutoff must be retained — the predicate is
strict `<` — and all three sibling tables must be untouched. An immediate re-run
must then delete nothing and report a skip with a structured reason, because a
second prune that quietly redid work would mean the first one had not finished.

##### Interrupted Run Stays Consistent

Stopping between chunks must leave every committed chunk deleted and no partial
row behind, and the next run must finish the job with no special handling — it
rescans, finds what is left, and drains it.

##### Watermark Advances At First Chunk

Stopping after the first chunk must leave `retention.watermark` already equal to
the run's cutoff, with every removed row strictly older than that value, so an
insert filter honouring the watermark cannot resurrect them.

##### Preflight Skip Leaves The Watermark

A failing delete-phase disk check must remove no row and leave the stored
watermark byte-identical, because advancing it with nothing deleted is
consent-free insert suppression — the one failure mode this design must not have.

##### Mid Run Failure Reports Partial

A free-space re-check that fails after some chunks have committed must stop
cleanly with `partial`, a populated `error_reason`, an empty skip `reason`, and
deleted counts matching what actually committed.

The audit record must be persisted on that error path with unchanged before/after
bytes, and the watermark, advanced at the first chunk, must still be advanced.

##### Non Conforming Rows Retained

Owned pre-cutoff rows whose timestamps fail the conformance guard must survive
the run and be reported in the result and the audit record, so a guard that
silently discarded them could not pass.

##### Delete And Vacuum Budgets Are Distinct

The delete budget must be satisfiable at a free-space figure the VACUUM 2× budget
rejects, proving the two checks are genuinely separate rather than one check
spelled twice.

Running at exactly that figure must complete with rows deleted and before/after
bytes unchanged — the "rows removed, bytes not yet reclaimed" outcome.

##### Live Rows Are Never Doomed

`source_key IS NULL` rows on both sides of the cutoff must be absent from the
scan's doomed set and present in full after the run, which is the delete-side
half of the live-rows invariant.

### View Query Reader Connections

Widget analytics reads use independent, disposable SQLite connections so read IPC does not serialize on the ingest writer mutex.

[[src-tauri/src/storage.rs#Storage#open_view_reader]] opens the resolved database path with `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX`, intentionally omitting URI interpretation, and retains the established 5-second busy timeout plus WAL-compatible transient-memory pragmas. Each reader closes after one response so WAL checkpoints and compaction are not starved.

The migrated view slice is [[src-tauri/src/storage.rs#Storage#get_context_savings_analytics]], [[src-tauri/src/storage.rs#Storage#get_token_history]], [[src-tauri/src/storage.rs#Storage#get_host_breakdown]], [[src-tauri/src/storage.rs#Storage#get_project_breakdown]], [[src-tauri/src/storage.rs#Storage#get_skill_breakdown]], [[src-tauri/src/storage.rs#Storage#get_hook_breakdown]], [[src-tauri/src/storage.rs#Storage#get_session_breakdown]], [[src-tauri/src/storage.rs#Storage#get_code_stats]], [[src-tauri/src/storage.rs#Storage#get_code_stats_history]], and [[src-tauri/src/storage.rs#Storage#get_llm_runtime_stats]]. Models aggregate, overview, history, and session reads use the same helper.

Multi-statement reads run in deferred transactions, giving one response a stable WAL snapshot while ingest commits concurrently. Cache probes and cache misses share that snapshot. SQL rows are fully materialized before parsing, project merging, downsampling, runtime turn walking, or history bucketing begins, shortening reader lifetime as well as removing writer-mutex contention.

[[view-reader-tests#View Reader Contention Tests]] pins the five-second slow-reader acceptance boundary under fast view queries and concurrent ingest.

Code-stat readers keep that close-before-shaping boundary while avoiding payload
allocation for migrated rows: SQL conditionally projects `full_input` only when
either stored line counter is NULL. On the frozen feature-020 corpus, the 30-day
history path measured 88.407 ms p95 across six release-harness samples against
its 300 ms budget; the protocol and integrity record live in
`specs/020-widget-query-perf/timing-measurement.md`.

### Schema

The database schema is versioned through migration 37 and includes usage, token, model analytics, context savings, learning, rule governance, session indexing, memory optimizer, code, runtime, retention aggregates, and metadata tables.

#### Usage Tracking

Tables for recording and aggregating provider-aware live usage bucket utilization over time.

- **usage_snapshots** — Raw live usage snapshots keyed by provider plus bucket key, with nullable source and account identity for CPA-managed credentials.
- **usage_hourly** — Hourly aggregates keyed by provider plus bucket key (avg/max/min utilization, sample_count). Unique on (hour, provider, bucket_key).

Live usage ingestion stores native and CPA-sourced buckets in the same tables. Migration 36 adds nullable `source`, `account_id`, and `account_label` columns; legacy NULL sources read as direct. Native cache reads exclude CPA rows, while CPA reads use provider, account, and bucket identity. CPA bucket keys use `cpa/{account}/{window}`, preserving hourly uniqueness and enabling complete CPA deletion from snapshots and aggregates.

Codex `rate_limits.resets_at` values are normalized from transcript epoch timestamps into RFC3339 strings before storage so the live pane can show the same reset countdown semantics as Claude. Migration 14 backfills older Claude-only rows by deriving stable bucket keys from legacy labels, and startup creates provider-only indexes after migrations so older databases can still boot before those columns exist. The generic `settings` table also stores Claude live-usage fetch metadata such as the last attempted poll time, any active 429 cooldown, and the configured indicator primary-provider preference used by the tray and indicator window.

The startup path restores recent live buckets from `usage_snapshots` through [[src-tauri/src/storage.rs#Storage#get_latest_usage_buckets]]. That lookup now uses a grouped latest-timestamp join instead of a correlated subquery because the older form could take tens of seconds once `usage_snapshots` grew large, which left the live pane stuck on `Loading…` during app startup.

##### Usage Snapshot Source Test Specs

Migration and persistence tests protect the source boundary between native and CPA usage data.

###### Migration 36 Account Dimension

Migration 36 must preserve legacy snapshots as direct usage, reopen idempotently, isolate CPA cache reads, and purge CPA rows from raw and hourly storage without deleting native rows.

#### Token Tracking

Tables for recording per-session token consumption and hourly host-level aggregates with provider provenance.

- **token_snapshots** — Raw token usage per provider/session (provider, session_id, hostname, timestamp, input/output/cache tokens, cwd). Indexed on provider-aware timestamp, session, and cwd paths.
- **token_hourly** — Hourly aggregates per provider/host (total tokens, turn_count). Unique on (hour, hostname, provider).
- Analytics session history, compact token stats, and delete-session cleanup all treat sessions as `(provider, session_id)` pairs so Claude and Codex ids cannot collide.

Migration 20 added `is_sidechain`, `agent_id`, and `parent_uuid` to `token_snapshots` for provider-agnostic sub-agent attribution; the [[backend#Tauri IPC Commands#Usage and Token Commands (14)]] `get_session_breakdown` rollup aggregates across all sidechain rows by `session_id` so a sub-agent's tokens count toward its parent session row. Hook-reported snapshots written before migration 20 stay tagged `is_sidechain=0` (a future CLI repair utility is documented as a TODO in [[src-tauri/src/storage.rs]]).

#### Model Analytics Evidence

Migration 28 stores replayable transcript evidence and source ownership for provider-qualified model analytics without a model catalog.

The source lifecycle and graph-resolution path is documented in [[data-flow#Model Observation Reconciliation]].

- **model_usage_observations** — Normalized turn and token facts with exact raw model identity, a nullable indexed `derived_model_id` attribution column, nullable token dimensions, resolved session/chain ownership, and source-local ordering.
- **model_observation_sources** — Retained source inventory, fingerprints, activity bounds, reconciliation status, and durable deletion suppression.
- **model_backfill_state** — Singleton progress and completeness state used to distinguish final empty claims from provisional recovered data.

Backfill lifecycle writes are transactional and state-guarded. Interrupted and explicit retry initialization advance the inventory generation, clear only run-local counters, and preserve evidence; pending work alone can become running. Root outcomes precede an explicit source-total publication marker, which distinguishes an authoritative empty inventory from work not yet inventoried. Batch counters cannot exceed remaining work, and only a failure-free resolved inventory with at least one configured root and a published source total can finish complete. Partial and failed states persist inventory completeness independently, so unreadable sources and unreadable roots remain distinguishable. Persisted diagnostics use the bounded `ModelBackfillDiagnostic` value rather than raw filesystem errors.

[[src-tauri/src/model_usage.rs#run_retained_model_history_backfill]] owns one retained-history pass under the shared process permit. It inventories each provider root off the async executor and commits its cumulative outcome before starting the next, then prepares stable generation-owned work before publishing the plan's validated source total. Bounded source batches commit and record progress between yields, source failures retain last-good rows, and only completed root proofs prune child rows before parents. Terminal `partial` versus `failed` reflects useful committed work, while inventory completeness depends only on resolved roots and attempted discovered sources.

Retention deletes model observations older than its confirmed cutoff with the transcript-owned targets, but retains each source inventory row so source reconciliation, revision detection, and cursor progress remain valid. [[src-tauri/src/storage.rs#Storage#replace_model_source]] re-reads the durable watermark in its replacement transaction and filters pre-cutoff `observed_at_ms` rows before inserting, preventing a revised or backfilled source from resurrecting pruned evidence. `usage_snapshots` and `token_snapshots` remain outside this policy: they are live-hook/API state without source replay semantics, and their bounded cleanup lifecycles remain independent.

[[src-tauri/src/lib.rs#run]] resets an interrupted running pass to a fresh `startup_resume` generation after storage initialization and reserves one nonblocking worker for migration-pending or resumed work. The reservation waits for live reconciliation to release the shared process permit instead of discarding historical work.

Migration 29 adds the `derived_model_id` column plus its index and re-arms `model_backfill_state` to pending under a bumped generation with a `migration` trigger, so existing installs re-attribute retained evidence on the next startup pass. Because attribution is computed only during parsing, the migration also nulls `mtime_ns` and `content_sha256` on active `ok` sources so [[src-tauri/src/storage.rs#classify_model_source_change]] cannot short-circuit them to fast/content-unchanged; the re-armed pass therefore genuinely re-parses pre-upgrade transcripts and stamps `derived_model_id` instead of leaving it null forever. [[src-tauri/src/model_usage.rs#apply_carry_forward_attribution]] stamps every parsed observation with its chain's running model — the last non-null, non-`<synthetic>` raw turn id; synthetic turns never update the running model, and rows before any model evidence stay null — while `raw_model_id` remains untouched replayable evidence. Overview, session paging, and session detail key attribution on `derived_model_id`; segment and switch semantics stay on raw turn evidence.

Attributed coverage uses one token-bearing observation population: rows with derived attribution form the numerator, and rows still null after carry-forward form the unattributed remainder. A zero-token denominator stays unavailable. Tokenless turns still contribute model, session, first/last-seen, primary-model, and switch evidence.

The overview and paged-session commands each open a short-lived read-only connection through [[src-tauri/src/storage.rs#Storage#open_view_reader]] and start their deferred transaction there, so neither waits on the primary storage mutex or serializes behind ingestion writes. The reader uses `SQLITE_OPEN_READ_ONLY`, in-memory temp storage, mmap, and a larger cache; it omits `query_only` because the overview needs temp tables.

##### Analytics Cache Primitive

The in-process analytics cache shares typed results only while every source table still has the version observed at insertion.

[[src-tauri/src/storage.rs#CacheKey]] identifies command-specific request dimensions and a 30-second wall-clock bucket so sliding windows naturally expire at a bucket boundary. [[src-tauri/src/storage.rs#get_or_compute]] checks the 45-second TTL and a [[src-tauri/src/storage.rs#TableVersions]] fingerprint made from indexed high-water markers. Model overview also fingerprints `rollup_meta.rollup_generation`; bucket and context commands keep their own probes. A failed probe bypasses the cache and computes fresh. Each [[src-tauri/src/storage.rs#Storage]] owns its three typed caches.

[[src-tauri/src/storage.rs#Storage#get_model_usage_overview]] serves the Models-page overview from one read-only deferred-transaction snapshot as a [[src-tauri/src/models.rs#ModelUsageOverviewResponse]]. During rollup build it preserves the established raw temp-table path. After completion, `scoped_overview` contains aggregate-grain closed UTC hours plus raw leading/current-hour rows, so it never rematerializes the full closed observation range; provider filtering remains sargable. The response carries: range totals (sessions, projects, turns, tokens, coverage); per-model reach rows (sessions, projects, turns, primary-in count, days active, tokens and share); a running-now entry per provider pairing the latest contiguous model run with its predecessor; fixed-bucket per-model distinct-session activity; a top-8 project × model session matrix; per-session model-count combinations with top co-occurrence pairs; and a parent-versus-subagent token split. Exact facet routing and non-decomposable exceptions are inventoried in [[backend#Backend#Database#Schema#Hourly Analytics Rollups]].

[[src-tauri/src/storage.rs#Storage#get_model_sessions]] pages sessions for one exact provider-qualified raw model without a catalog. Its checksummed opaque cursor fixes the half-open range, records the persisted model-data revision, and seeks by last activity descending, then binary provider/session identity ascending. Observation replacement, source pruning/failure visibility, deletion suppression, and model cwd changes advance that revision in their own commit; a later page rejects a stale cursor instead of mixing totals with unreachable rows. Each page derives selected-model usage, latest project/host context, range-scoped primary model, distinct models, independent chains, and turn-only within-chain switches from unsuppressed evidence.

[[src-tauri/src/storage.rs#Storage#get_session_model_history]] reads one provider/session over the same half-open range and unsuppressed source ownership. It returns coverage totals from every token-bearing observation, but only ordered turns create segments: repeated models compress, null-model turns compress into gaps and reset adjacency, and token-only rows neither create segments nor reset switches. Parent and subagent chain metadata must remain consistent, chains order parent first then first activity and binary chain ID, segment endpoints are the inclusive first/last turn timestamps, and primary-model ties use attributed tokens, turn count, then binary provider/raw ID. A session with no retained in-range observations returns a distinct storage-level not-found outcome for stale-row IPC handling.

Existing session, project, and host deletion transactions select model evidence through retained source ownership, delete observation children first, and leave each matching source suppressed at its last committed content hash. Session deletion matches the provider-qualified analytics/root session so parent and subagent sources are removed together. Project deletion selects a source when its retained `cwd` or any child observation `cwd` matches, then deletes every child of that source to preserve atomic replacement. Unchanged retries remain suppressed; only an atomic changed-content replacement restores evidence. Project rename updates exact-matching source and observation `cwd` fields independently in one transaction, preserving per-record cwd differences. Tauri wrappers perform snapshot reads and mutations off the async command worker, then emit `model-analytics-updated` only after commit. Model evidence has no independent TTL and is not coupled to token-hourly cleanup.

#### Model Analytics Test Specs

Behavior specs for the model-analytics parse and query paths, each covered by exactly one `// @lat:` reference at its representative test.

These unit specs exercise the current working-tree parsers and storage queries directly, without a live Tauri app. They complement the lifecycle prose above by pinning the exact edge-case counters, reconstructed token numbers, and suppression exclusions that the reconciliation and read paths must preserve.

##### Claude Transcript Adapter Edge Cases

[[src-tauri/src/model_usage.rs#parse_claude_model_usage_jsonl]] never lets one bad record abort a source; each edge case degrades to a counter or a null field instead.

Covers a truncated final line that preserves prior observations, sidechain turns missing an agent id, negative epoch timestamps, missing or non-string `type`, invalid token dimensions that still emit an unavailable-token observation, and model-id whitespace trimming where blank or missing ids stay null with no `unknown` synthesis.

##### Codex Cumulative Delta Reconstruction

[[src-tauri/src/model_usage.rs#parse_codex_model_usage_jsonl]] rebuilds per-turn deltas from cumulative `token_count` totals without underflowing or inventing identity.

Asserts exact input and cache-read decomposition across three monotonic turns, a decreasing counter that resets its baseline and emits the reset diagnostic, a trailing `session_meta` that still attributes earlier records under the two-pass invariant, and `turn_context` model evidence kept separate from token-only deltas.

##### Model Sessions Cursor Codec

[[src-tauri/src/storage.rs#encode_model_sessions_cursor]] and its decoder round-trip every cursor field and reject tampered cursors.

A flipped checksum nibble and a truncated envelope both decode to an [[src-tauri/src/storage.rs#ModelSessionsQueryError]] invalid-cursor error rather than a mismatched or partial page.

##### Model Hourly Ingest Fold Exactness

The model replacement transaction must leave its unpruned hourly rollup exactly equal to a raw source group-by and must roll every fold mutation back on failure.

Seeded observations span UTC hours, model identities, missing and invalid attribution, explicit zeroes, and absent token dimensions. Replacement refolds rather than doubles counts and advances `rollup_generation` once. Forced metadata failure and SQLite integer overflow both roll raw rows, rollups, source metadata, and generation back atomically.

##### Model Hourly Ingest Fold Burst Budget

The model ingest fold must add no more than 10% p95 latency to representative burst-shaped source replacement batches.

The ignored release-mode benchmark times 25 replacements of one 6,000-row batch with production folding against the same transaction with only folding disabled. Parsing, fingerprinting, initialization, and warm-up remain outside the measured region.

##### Cache Probe Cross-Connection and Cost Spike

The model cache probe records an absolute high-water timestamp, so it observes
commits from independent SQLite connections without a connection-local counter.

The normal test proves an independent writer's committed insert changes the
probe. The ignored 250,000-row diagnostic measures both count-plus-max and
max-only probes against a representative guarded aggregate. Count-plus-max
exceeds the 5% budget, so the selected max-only probe must remain below it;
deletes are bounded by the cache TTL.

##### Rollup Generation Cache Invalidation

Rollup-dependent cache fingerprints must change when an existing hourly row is updated without advancing any raw-table or rollup-row maximum.

The regression warms a model cache entry, commits an independent-connection UPSERT plus generation bump, proves raw COUNT/MAX and rollup COUNT/MAX stay fixed, and requires the next probe to recompute.

##### Hybrid Model Overview Parity

Completed model-rollup reads must equal the established raw computation across overview facets, while incomplete states remain on raw evidence.

The seeded comparison pins an unaligned endpoint and covers closed and open hours, missing and explicit-zero evidence, suppression, raw-pruned authority, and completed-empty `buildingIndex` state.

##### Bounded Completed Model Raw Seeks

Completed rollup reads must seek only exact raw boundary intervals and scoped sessions instead of scanning the full observation window.

SQLite 3.45 plan checks require activity residual ranges, project prefix/tie lookups, and paged running-turn reads to use bounded indexes without raw scans or temporary ordering.

##### Running Model Pagination Parity

Paged running-model reads must preserve the complete turn order across page boundaries and cannot stop until each represented provider finds a predecessor or exhausts its range.

More than 1,024 same-prefix turns put decisive ordinal and binary-key ties on the second SQL page, while an interleaved provider with one model verifies exhaustion, oldest `runningSinceAt`, and a NULL predecessor.

#### Hourly Analytics Rollups

Migration 37 creates source-keyed rollup storage for bounded model and runtime reads while preserving raw evidence.

- **model_usage_hourly** — One row per UTC hour, provider, derived model, and source. It denormalizes analytics session identity; stores observation, turn, token, sidechain, and NULL-aware token-dimension counts and sums; retains first/last evidence bounds and a `raw_pruned` marker. [[src-tauri/src/storage.rs#Storage#replace_model_source]] inserts retained raw evidence, then groups it into this table with one SQLite `INSERT SELECT` in the same replacement transaction. A non-colliding empty derived-model key represents raw NULL attribution because normalized provider model ids cannot be empty.
- **runtime_hourly** — One row per UTC hour, provider, and source. It denormalizes session identity and stores finalized turn count, runtime seconds, turn bounds, and a `raw_pruned` marker. [[src-tauri/src/storage.rs#refold_runtime_source]] preserves the 5-minute logical-turn continuity rule; ordinary longer gaps close at the prior event, while an `asst_tool_use` followed by `user_tool_result` contributes its persisted gap clamped to 6 hours. Each closed turn belongs wholly to its start UTC hour.
- **runtime_turn_state** — One row per provider/source finalization bookmark, with the highest finalized raw rowid and the remaining open logical turn's start. Open turns are not folded into `runtime_hourly`.
- **rollup_meta** — Singleton generation plus independent model/runtime backfill status and resumable bookmarks.

Both hourly tables lead their unique key with `hour_utc` for bounded range reads and carry a `(provider, source_key)` index for exact source invalidation. Model replacement deletes that source's prior `raw_pruned=0` rows before refolding and preserves NULL-aware token evidence; a conflict with `raw_pruned=1` is never updated because that whole bucket is already authoritative. Transcript replacement likewise deletes only unpruned runtime rows plus state and refolds from newly persisted events. Explicit project, host, session, and completed-root deletion removes every matching hourly row and runtime state beside raw detail; suppression alone remains a read-time source join. Each mutation shares one raw transaction, so a later failure rolls raw, rollup, state, source metadata, and generation back together. Migration 37 itself initializes only the metadata singleton; it performs no raw-table fold or backfill. Recording v37 is a one-way door because older builds refuse it through `SCHEMA_TOO_NEW`, but the additive DDL preserves every pre-upgrade row.

Retention prevalidates every doomed model group against its full raw group-by and every doomed runtime source against the deterministic refold before advancing the watermark. A mismatch refuses the run without deletion. Each delete chunk promotes covered hourly rows to `raw_pruned=1` in the same transaction as raw deletion. Runtime selects finalized events at or below the persisted bookmark. A trailing turn stays raw unless every event is doomed and any tool wait is fully realized before the cutoff; only then is it sealed into hourly authority. The bookmark clamps past removed rowids. Runtime stats never merge `retention_daily_aggregates`; those daily event counters remain available only to transcript/code views.

Completed model reads use hourly rows for decomposable facets: represented providers; scoped sessions/evidence; turn, token, and distinct-model totals; per-session/model reach, turns, tokens, primary model, combinations, and pairs; active days and first/last evidence; delegation totals; and history sums. Every row joins active `model_observation_sources` during the read, so suppression is immediate, and the empty model sentinel returns to NULL attribution before aggregation.

Three facets need narrower handling because migration 37 omits their full grain. Activity/history use a rollup hour only when that whole UTC hour fits one existing response bucket; explicit half-open raw intervals seek only bucket-crossing hours. Completed overview scope and provider discovery express leading and trailing raw edges as separate bounded `UNION ALL` branches, never one full-window range with an `OR` edge filter. Project labels seek descending time/ordinal prefixes per scoped chain, fetch exact-prefix cwd ties without SQL sorting, and compare full binary suffixes in Rust while retaining source-cwd fallback and older-prefix continuation. Active source cwd and any pruned-authority keys are materialized once per read. Running-now pages the observed-time index, retains complete timestamp/provider groups across pages, applies the remaining order in Rust, and stops after each represented provider finds its first differing model or the range is exhausted. These exceptions do not rematerialize closed historical observations in the overview temp table.

##### Chunked Rollup Backfill Framework

[[src-tauri/src/rollup_backfill.rs#run_rollup_backfill]] safely drives target-specific model and runtime folds without owning either rollup's SQL or exposing a command surface.

Each chunk receives a 250 ms ceiling and row limit, runs its fold plus durable bookmark update in one immediate transaction under the shared ingest permit, then releases the permit before `wal_checkpoint(TRUNCATE)`. The transaction deadline starts only after that permit is acquired, so a maintenance lease never consumes the fold budget. A disk preflight precedes every chunk.

Checkpoint busy/failure and unreadable or insufficient disk space stop as typed terminal errors. A committed bookmark remains resumable if its following checkpoint fails; unexpected target SQL and invariant failures bubble to the caller and emit a generic failure detail instead of claiming a checkpoint fault.

The runtime target prepares one source-keyed `session_events` block at a time outside the ingest permit, sorting and folding it in memory. Current source replacement writes one contiguous rowid block; the compact transaction revalidates its bounds, count, identity, content hash, and generation before replacing only unpruned rollup/state rows and advancing the source-end bookmark. Preparation can exceed the row target without extending the immediate transaction; compact commits still enforce the 250 ms ceiling. `raw_pruned=1` rows remain authoritative, the first commit reconciles live folds, and empty databases complete with a null terminal bookmark.

The model target selects a row-budgeted half-open UTC-hour range before admission, then re-reads current raw evidence inside the permit transaction. It replaces only that range's `raw_pruned=0` groups and atomically stores the inclusive end-of-hour bookmark; live replacement refolds old hours directly, so rows arriving behind the bookmark remain exact. Empty databases commit `complete` immediately.

[[src-tauri/src/lib.rs#spawn_model_rollup_backfill]] schedules incomplete model state after startup. [[src-tauri/src/lib.rs#rebuild_model_rollup]] reserves one target run, refuses active or queued maintenance without waiting, clears only raw-backed rows and bookmark state, then uses the same runner. Per-run progress and finished events are emitted only after their lifecycle transaction commits; run ids prevent delayed events from an older rebuild replacing current UI state.

[[rollup-concurrency-tests#Rollup Backfill Concurrency Test Specs]] exercises concrete model and runtime targets across maintenance deferral, live ingest, exact resume, and real WAL truncation boundaries.

###### Backfill Interrupt And Exact Resume

An interruption after a committed chunk must resume strictly after its atomic bookmark, producing every source row exactly once without a duplicate or gap.

###### Maintenance Lease Acquires Between Chunks

A queued maintenance writer must acquire the fair ingest gate within one 250 ms chunk bound, and the backfill must remain yielded until that lease releases.

###### Disk And Checkpoint Failures Stop Safely

Disk refusal must advance no bookmark, while checkpoint busy or failure must stop with a typed terminal and preserve the already committed bookmark for exact resume.

##### Rollup Migration Test Specs

Migration tests pin the additive schema boundary before fold and backfill behavior lands.

###### Migration 37 Additive Schema

Fresh and v36 databases must gain every rollup table and invalidation index, preserve seeded raw evidence, leave both hourly tables empty, initialize pending bookmarks, and reopen with one v37 record.

#### Learning System

Tables for the behavioral learning pipeline: observations, summaries, analysis runs, and discovered rules.

- **observations** — Tool-use observations (provider, session_id, hook_phase, tool_name, tool_input/output, cwd). Indexed on session_id, timestamp, created_at, and provider cleanup paths.
- **observation_summaries** — Per-period/provider/project summaries (tool_counts JSON, error_count, total). Unique on (period, provider, project). Feature 005 (US5 T062, M-1) makes this formerly write-only table readable via `Storage::get_observation_summaries` and folds it into `get_observation_sparkline` as the post-retention historical tail so the trend survives observation pruning; the same change tightens the summary `error_count` from a bare `%error%` substring to a structured-failure-marker predicate (JSON `is_error`/error/status keys, leading `Error:`, runtime panic/traceback banners).
- **learning_runs** — Analysis run records (trigger_mode, observations_analyzed, rules created/updated, duration, status, error, inference_metadata). Feature 005 (US5 T058, H-6) decodes `inference_metadata` tolerantly in `get_learning_runs` into the derived `RunInferenceSummary` rollup on `LearningRun` (no migration — column added by migration 24); NULL/parse-error/empty ⇒ `None`.
- **learned_rules** — Discovered patterns (name unique, domain, confidence, observation_count, file_path, content, state, is_anti_pattern, source). The `content` column (migration 11) stores sanitized rule text for manual promotion. Migration 25 (feature 005) adds governance columns `lifecycle` (persisted lifecycle state, distinct from the read-derived `state` quality label), `origin_run_id`/`origin_model`/`origin_at` (provenance), `current_version`, and `superseded_by`.

Migration 25 also adds six rule-governance tables for the hardened learning loop: **rule_versions** (append-only content history enabling rollback), **rule_evidence_citations** (retention-proof denormalized evidence snapshots grounding a rule), **rule_tombstones** (name-keyed durable suppression that survives re-extraction), **operator_feedback** (per-rule maintainer accept/reject/bad — the primary outcome signal), **evaluation_results** (counterfactual replay verdicts linked to rule + run), and **reviewer_overrides** (audited approval of a regressing rule).

Observation retention (`cleanup_old_observations`) is feature-005-hardened (US5 T061, M-2 / SC-010): the delete cutoff is `MIN(analyzed_watermark, now - 30d)` where `analyzed_watermark = MAX(created_at) FROM learning_runs WHERE status IN ('completed','degraded')`. Observations newer than the watermark have not had an analysis opportunity and are never deleted; with zero completed/degraded runs nothing is deleted at all; the 30-day safety floor only ever adds retention. The summarize-then-delete pair runs in one transaction so a failed summary write rolls back the delete (no more best-effort `.ok()` then unconditional delete).

Startup also creates covering observation indexes for `(created_at, tool_name)` and `(provider, created_at, tool_name)` so learning UI queries such as `get_top_tools` can stay on exact raw-observation windows without paying extra table scans. The same startup pass adds `tool_actions` indexes for `(category, timestamp)` and `(category, provider, session_id)` so ordered code-history lookups and per-session code aggregations avoid broad category scans.

#### Session Indexing

Stores detailed tool invocation and response-time data extracted from transcripts, which backs the in-app code and session analytics.

Neither table backs session *search*: search is Tantivy over indexed session
messages ([[backend#Backend#Session Indexing]]), and the HTTP `/api/v1/sessions/search`
path the MCP server calls never reads `tool_actions`. The distinction matters
because retention prunes these two tables and leaves the full-text index alone —
a search hit therefore survives the deletion of the rows behind its code stats,
which is precisely the degradation
[[frontend#Frontend#Components#Retention Degradation]] exists to state.

- **tool_actions** — Tool invocation details behind `get_code_stats`, `get_batch_session_code_stats` and sub-agent discovery (provider, message_id, session_id, tool_name, category, file_path, summary, full_input/output, plus `is_sidechain`, `agent_id`, and `parent_uuid` from migration 20, and nullable `lines_added`/`lines_removed` from migration 33). Indexed on provider/session, message_id, file_path, category, and the new provider+session+sidechain / provider+session+agent pairs. `full_input` is truncated to 10KB, so the `lines_added`/`lines_removed` counts for `code_change` rows are computed at ingest from the untruncated input. The code-stats queries select those persisted counters directly and conditionally project `full_input` only when either counter is NULL; these legacy rows keep the tolerant parser while migrated rows never materialize the payload. Retained transcript rows are committed only through source-owned snapshot replacement, and rows written that way with `category = 'tool_detail'` carry NULL in both payload columns — see [[backend#Backend#Database#tool_detail payload carve-out]].
- **response_times** — Assistant response latency per provider/session turn (provider, session_id, timestamp, response_secs, idle_secs, plus the same migration-20 `is_sidechain`/`agent_id`/`parent_uuid` triple). Unique on (provider, session_id, timestamp).

#### Retention aggregates

Migration 35 preserves queryable shape after source-owned transcript detail is pruned.

- **retention_daily_aggregates** — Per-provider/source/session/day counters for
  tool calls, session events, code changes, added/removed lines, agent identity,
  and changed-file path. Its primary key merges one pruning chunk with another
  without retaining payloads, event keys, or message bodies.

The delete engine writes this table before deleting each chunk, in the same
transaction. Code-stat range/history and per-session reads merge its counters
with surviving raw rows, while session breakdown/tree reads use its agent and
tool-call counts. Source suppression and root pruning delete matching aggregate
rows with their source-owned detail; snapshot replacement leaves already-pruned
history intact because the retention watermark prevents an old replay from
resurrecting raw rows.

Migration 35 creates the table and indexes idempotently so a schema-version
rewind can record version 35 without colliding with already-current objects.

#### Skill Usages

Recognized `SKILL.md` loads derived during the same Session Indexing extraction pass, keyed for analytics drilldowns by skill, provider, project, and host.

- **skill_usages** — One row per recognized skill load (provider, session_id, message_id, skill_name, skill_path, timestamp, tool_name, cwd, hostname). Unique on (provider, session_id, message_id, skill_name, skill_path, timestamp). Indexed on provider+timestamp, provider+session, skill+timestamp, and the migration-22 skill+cwd pair that powers per-project drilldowns. Migration 23 re-arms `skill_usage_reingest_pending` so historical sessions are replayed against the updated extractor without any schema change.

[[src-tauri/src/sessions.rs#extract_skill_accesses_from_tool_action]] recognizes three ingest shapes: Codex `exec_command` calls that read a `SKILL.md` path with `cat`/`head`/`tail`/etc., Claude `Read` calls against a `SKILL.md` path, and Claude `Skill` tool calls. The `Skill` arm normalizes the `skill` input via [[src-tauri/src/sessions.rs#skill_access_from_skill_tool_input]] by stripping any `plugin:` prefix so Claude rows merge with Codex's bare folder names (e.g. Claude `superpowers:using-superpowers` collapses onto Codex `using-superpowers`), and synthesizes a `skill://<raw>` path that preserves the original identifier for forensic drilldowns without colliding with filesystem paths.

`cwd` and `hostname` are populated in source-owned snapshots: Claude pulls `cwd` from each record's top-level field, Codex threads session-level `cwd` through every tool message in [[src-tauri/src/sessions.rs#ExtractedMessage]], and reconciliation captures the local hostname once per source. The HTTP message-ingest path leaves skill usage empty because its flattened payload has no tool-action detail.

#### Hook Invocations

Hook fires remain durable audit history for the Hooks breakdown; current subagent counts instead use a bounded process-local lifecycle fold and never replay this table.

- **hook_invocations** — One row per observed hook fire (provider, session_id, chain_id, parent_chain_id, agent_id, is_sidechain, timestamp, hook_event, hook_matcher, tool_name, hook_identity, script_command_raw, exit_code, duration_ms, cwd, hostname, message_id). Owned hooks are unique on `(provider, source_key, chain_id, timestamp, hook_identity)`; source-less hooks use `(provider, session_id, chain_id, timestamp, hook_identity)`. Live subagent lifecycle rows use `agent_id` as `chain_id`, preserving same-time siblings independently. Migration 30 rebuilds retained Claude rows through source reconciliation while preserving deduplicated source-less observations.

[[src-tauri/src/sessions.rs#extract_hook_invocation_from_attachment]] recognizes records whose `attachment.type` begins `hook_` (covering `hook_success`, `hook_failure`, `hook_timeout`, `hook_blocked`) and maps `hookEvent`, `hookName`, `command`, `exitCode`, and `durationMs` onto the row. The Claude attachment's matcher half of `hookName` (e.g., the `Bash` in `PreToolUse:Bash`) becomes `hook_matcher`, and when the event is `PreToolUse` or `PostToolUse` it also fills `tool_name` so per-tool breakdowns work without a separate column lookup.

[[src-tauri/src/sessions.rs#canonicalize_hook_identity]] forms the aggregation key: paths inside `~/.config/quill/scripts/` or `~/.config/quill/codex/scripts/` collapse to `quill:<basename>` so Quill-managed rows have stable per-machine identities, `${CLAUDE_PLUGIN_ROOT}/<dir>/<file>` is kept verbatim because the unexpanded env-var prefix is the only stable plugin-scoped identifier the transcript provides, any other absolute path is reduced to its basename, and records with no `command` (older Claude transcripts) fall back to `hookName`. The verbatim command is preserved in `script_command_raw` (truncated to 2048 chars at a UTF-8 boundary) for forensic drilldowns.

Live Claude and Codex rows are inserted by [[src-tauri/src/storage.rs#Storage#store_hook_observation]] from the `POST /api/v1/hooks/observed` background task. Codex identity remains event-scoped (`hook_event` with optional `:tool_name`) because its generic observer cannot identify sibling scripts. Claude uses its existing `observe.cjs` for the four root/subagent lifecycle groups. Both managed paths are gated by the existing `activity_tracking` feature and preserve user-owned hook configuration.

The provider-neutral endpoint accepts the shared 11-event set and length-caps identity fields. [[src-tauri/src/models.rs#ObservedHookObservation]] adds optional hostname and `SessionStart.source` while keeping old payloads audit-compatible. Complete lifecycle evidence is folded synchronously by [[src-tauri/src/server.rs#ObservedSubagentState]] before background SQLite persistence; a state change emits `hooks-observed-updated` even if the audit write later fails. An accepted Claude `SubagentStart` also schedules coalesced changed-source scans near 10 and 30 seconds, feeding only exact transcript-derived child models into retained model reconciliation. Its committed `model-analytics-updated` event invalidates Sessions directly, so naming does not wait for another hook event.

The in-memory registry keys roots by provider, normalized hostname, and root session, then agents by `agent_id`. It retains at most 1,024 roots and 256 agent lifecycles per root. Startup/resume/clear/fork establish coverage, compact preserves it, end clears it, and tracking/provider changes invalidate it. Root starts, compaction, agent transitions, and supported Codex activity refresh the root activity timestamp. Reads invalidate an active root after 15 minutes without qualifying activity, five default three-minute usage-poll intervals, so lost parent-end delivery becomes null rather than a false ended zero. Missing identity, unsupported sources, ordering ambiguity, saturation, restart, lost coverage, or an authenticated pre-fold rejection also returns null for the identifiable exact root.

Known limitation (Claude side): transcript extraction still sees only `hook_*` attachments for non-lifecycle Hooks-breakdown evidence. The managed live observer closes the four lifecycle groups needed by Sessions counts, not every silent Claude hook.

#### Working Context Store

The MCP context store keeps large transient context out of the analytics database.

The Python MCP tools in [[src-tauri/claude-integration/mcp/tools/context.py]] create `~/.config/quill/context/context.db` with `sources`, `chunks`, `executions`, `continuity_events`, `compaction_snapshots`, and `fetch_cache` tables. SQLite FTS5 is used when available, with a LIKE fallback so older SQLite builds still search indexed chunks. The retired MCP continuity write and snapshot-read path no longer exposes a tool; `continuity_events` and `compaction_snapshots` persist only as inert historical storage. Context data stays on the machine running the MCP server.

#### Context Savings Events

The main analytics database stores compact context-savings telemetry from local and remote providers.

- **context_savings_events** — Append-only event records keyed by `event_id`, with provider, session, host, cwd, event type, source, decision, **category**, byte counts, approximate token estimates, refs, and bounded metadata.

Every event carries a `category` from a closed taxonomy: `preservation` (content written to the MCP context store and kept out of the LLM transcript), `retrieval` (LLM pulled preserved content back via `quill_get_context_source` or compaction snapshot read), `routing` (text injected into the transcript by router/capture guidance, search snippets, or bounded `quill_execute` results — these are *transcript cost*, not savings), and `telemetry` (hook observations like `capture.event` and `capture.snapshot` that record session activity but neither leave nor enter the transcript). The canonical mapping lives in [[src-tauri/src/context_category.rs#derive_category]] and is mirrored by `deriveCategory` in `src-tauri/claude-integration/scripts/context-telemetry.cjs` and `_derive_category` in [[src-tauri/claude-integration/mcp/tools/context.py]]; producers set `category` explicitly per call site, the server derives it from `(eventType, decision)` only as a fallback for legacy callers via [[src-tauri/src/context_category.rs#derive_category]], and [[src-tauri/src/storage.rs#backfill_context_event_categories]] applies the same mapping to historical rows during migration 18. Migration 19 re-runs that backfill and zeroes saved/preserved token fields for non-preservation/retrieval rows so stale telemetry producers cannot pollute event-level displays.

The HTTP server accepts batches from context hooks and MCP tools, deduplicates with `INSERT OR IGNORE`, and emits `context-savings-updated`. Analytics queries aggregate by time bucket, provider, category, event type, source, decision, and cwd for the Context tab while leaving large source content in the MCP context store. The shared `CONTEXT_SAVINGS_AGGREGATES_SQL` fragment sums byte and token-indexed/returned columns across every event so breakdown rows still surface router and telemetry traffic, but the saved and preserved token columns inside the same fragment are gated to `category IN ('preservation', 'retrieval')` so capture-hook telemetry contributes zero. The summary path additionally runs `CONTEXT_SAVINGS_CATEGORY_TOTALS_SQL` for the four headline figures (preserved, retrieved, routing, telemetry-event-count) and `CONTEXT_SAVINGS_RETENTION_SQL` to compute `retention_ratio = sources_retrieved / sources_preserved` over distinct `source_ref` values that fall in the active window — both events must be in-window so the ratio stays bounded in `[0, 1]` and reflects engagement rather than pre-window leftovers.

#### Memory Optimizer

Tables for tracking memory files, optimization runs, and actionable suggestions with lifecycle management.

- **memory_files** — Tracked memory files (project_path, file_path, content_hash, last_scanned_at). Unique on (project_path, file_path).
- **optimization_runs** — Optimization run records (project_path, trigger, memories_scanned, suggestions_created, status, timestamps).
- **optimization_suggestions** — Suggestions with lifecycle (run_id FK, action_type, target_file, reasoning, proposed_content, status, backup_data, group_id). Indexed on run_id, project_path+status, group_id.

#### Source-Owned Transcript Analytics

Migration 30 establishes source and chain identity for transcript-derived analytics while preserving live rows as source-less data.

`session_events`, `response_times`, `tool_actions`, `skill_usages`, and `hook_invocations` carry nullable `source_key`, required resolved-root `session_id`, required native `chain_id`, and nullable `parent_chain_id`. Events and tool actions also carry stable `event_key` and `action_key` identities; missing tool IDs fall back to message/block identity or source-record/block ordinals. Partial unique indexes separate source-owned replay identity from source-less live identity, and each table has a `(provider, source_key)` lookup index.

Owned/live identities are respectively: `session_events` `(provider, source_key, event_key)` / `(provider, session_id, event_key)`; `response_times` `(provider, source_key, chain_id, timestamp)` / `(provider, session_id, chain_id, timestamp)`; `tool_actions` `(provider, source_key, action_key)` / `(provider, session_id, action_key)`; `skill_usages` substitutes `source_key` or `session_id` before `(message_id, skill_name, skill_path, timestamp)`; and `hook_invocations` substitutes the same owner before `(chain_id, timestamp, hook_identity)`.

`transcript_analytics_sources` stores canonical root/path ownership, fingerprints, last-good native and resolved identity, origin, inventory generation, processing diagnostics, and durable suppression. `live_analytics_sessions` stores project, cwd, and host origin for source-less analytics. Migration 30 sets `transcript_analytics_reingest_pending` for the later retained-source rebuild.

Migration 30 renames the five prior analytics tables to `*_legacy_v30` and rebuilds them around source identity. The archives are **retained**, not dropped: rows with no hostname, and local rows whose transcript Claude has since pruned, are neither provably remote nor guaranteed rebuildable, and retention beats copying a multi-GB database. Nothing queries an archive and no index is kept on one — legacy named indexes are dropped so a retained archive cannot collide with the rebuilt tables' index names. An archive holding no rows at all is dropped immediately, so fresh installs stay clean.

Carry-forward is limited to rows this machine can never rebuild from a local transcript. `hook_invocations` carries forward `provider = 'codex'` (Codex rollouts are never transcript-derived) or any row stamped with a hostname other than the local one, folding v29's `agent_id`-qualified duplicates onto the lowest rowid. `skill_usages` carries forward on the remote-host predicate alone. Only those two tables have a `hostname` column (migrations 22 and 27), so the other three cannot discriminate and retain everything in the archive. Every carried-forward session with a known hostname gets one `live_analytics_sessions` origin row, because project and host deletion reach source-less rows only through recorded live origin.

Migration 31 adds `project_path_renames` plus `(provider, chain_id, timestamp)` runtime ordering support. The manage-data rename command and the ingest-side rename resolution that consulted this table were later removed, so the table persists only as inert schema history.

[[src-tauri/src/lib.rs#run]] schedules whole-root transcript reconciliation during app setup, independently of Session Search or Analytics-window mounting. Blocking inventory and parsing run in the background under the same provider/root permits as live source work. An existing empty root proves an empty inventory; a missing, unreadable, or unavailable configured root is incomplete and cannot authorize pruning or marker clearance.

Reconciliation compares canonical source key/path and last-good status before reading content. Matching `mtime_ns` plus size advances only `seen_generation`; a changed fast fingerprint hashes one stable read and likewise preserves all five tables when the stored hash matches. Only new, changed, failed, or root-restamped sources parse and replace. Failures persist bounded retry diagnostics without changing the last-good fingerprint, identity, or child rows. While `transcript_analytics_reingest_pending` is set both short-circuits are bypassed, so an interrupted rebuild genuinely replays every retained source instead of trusting a stale fingerprint; the marker clears only after every root supplies complete inventory and prune proof.

[[src-tauri/src/storage.rs#Storage#refresh_unchanged_transcript_analytics_sources]] advances every unchanged source of one root in a single transaction rather than one per source — a real corpus collapses roughly 5,500 transactions into one. It returns the source keys whose rows did not update because the root generation moved under a concurrent run, so callers keep per-source stale-generation handling instead of one aggregate verdict; the single-source method is a thin wrapper over it.

`replace_transcript_analytics_snapshot` replaces all five owned analytics tables and the source registry in one transaction; valid empty snapshots remove only that source, while suppression and any insert failure leave prior rows intact. Owned inserts use `INSERT OR IGNORE` through statements prepared once outside their loops, matching the source-less live paths — an owned identity is the table's own dedupe key, so a legitimate repeat must not roll back the whole five-table snapshot. Distinct `cwd` values are resolved through the rename map once into a lookup table instead of once per skill and hook row. Registry upserts advance `seen_generation`; stale prepared generations are rejected before owned rows change. Parse or identity conflicts retain last-good registry state. The `session_events` and `tool_actions` insert loops are additionally filtered by [[backend#Backend#Database#Insert-time watermark filtering]], which is what stops this delete-and-reinsert from resurrecting pruned history, and the `tool_actions` loop applies the [[backend#Backend#Database#tool_detail payload carve-out]] to the rows that do land.

[[src-tauri/src/storage.rs#Storage#store_live_session_analytics]] atomically writes source-less runtime or hook rows with durable project, full-cwd, and host origin. Origin upserts preserve known fields with `COALESCE`; live event rows require unique message UUID identity and always use the incoming session as both root and chain.

Session, project, and host deletion removes all five analytics tables in one transaction. A retained project/host match expands through provider-qualified analytics roots before leaving every sibling source as a suppressed tombstone. Source-less project/host rows use only exact recorded live origin; direct session deletion also catches unmapped legacy live rows. Committed deletions emit `transcript-analytics-updated` only when transcript rows changed.

#### Transcript Analytics Test Specs

Behavior specs for the source-owned transcript pipeline — migrations, snapshot replacement, freshness classification, and identity resolution — each covered by exactly one `// @lat:` reference at its representative test.

These unit specs run against a real migrated SQLite file and real on-disk JSONL fixtures, without a live Tauri app. They pin the invariants the prose above states as guarantees: that a failed write leaves last-known-good rows intact, that a superseded generation cannot overwrite newer data, that carry-forward is limited to unrebuildable rows, and that identity resolution degrades to a counter instead of discarding a source.

##### Owned Snapshot Replacement Atomicity

[[src-tauri/src/storage.rs#Storage#replace_transcript_analytics_snapshot]] is one transaction across all five owned tables plus the registry, so a failure at the last statement must undo every delete and insert before it.

A replacement that violates the registry `CHECK` after the owned tables were already rewritten must restore the prior rows exactly, leave a sibling source of the same root untouched, and leave the registry generation unadvanced. The positive half asserts the other edge: an empty snapshot is a legal replacement that clears exactly one source and no sibling.

##### Snapshot Generation Guards

A snapshot prepared against generation `G` must never overwrite rows once the root has moved past `G`, however the advance was recorded.

Both paths are covered: the root generation setting advancing under a concurrent run, and a registry row already stamped newer than the replay. Each returns the `StaleGeneration` verdict rather than an error, leaves prior owned rows byte-identical, and does not restamp the registry.

##### Migration 30 Carry-Forward Scope

Migration 30 must carry forward exactly the rows this machine can never rebuild from a local transcript, and nothing else.

Codex hook rows and any row stamped with a foreign hostname survive as source-less data; local Claude rows, and rows with no hostname at all, exist only in the retained `*_legacy_v30` archives. Migration 34 intentionally deletes the dead `tool_actions_legacy_v30` archive, so it is the exception. The test also pins the v29 `agent_id` duplicate fold and the `live_analytics_sessions` origin row registered per carried-forward session, without which project and host deletion could never reach those rows.

##### Migration 30 Idempotence

Reopening a database that already ran migration 30 must skip it rather than renaming the rebuilt tables a second time.

The version-gated loop records exactly one `schema_version` row for the migration and reaches the same final version on a re-open, which is the only thing standing between a normal restart and a second destructive rebuild.

##### Migration 31 Idempotence

Reopening a database that already ran migration 31 must not re-enter it.

Migration 31 creates the rename table and the native-chain runtime index; re-entry must be a no-op that records no second `schema_version` row.

##### Migration 32 Idempotence

Reopening a database that already ran migration 32 must not re-enter it.

Migration 32 only creates the covering runtime index, but the same gate protects it, and a second recorded version row would misreport the schema ceiling.

##### Empty Legacy Archive Cleanup

A fresh install renames five empty tables and has no history worth keeping, so migration 30 must drop those archives instead of leaving dead tables in every new database.

Retention is a recovery mechanism for existing corpora only; without this the schema of every new install would permanently carry five unqueried tables.

##### Schema Ceiling Refusal

A database written by a newer build cannot be downgraded in place, so `Storage::init` refuses it instead of starting an app that would silently record nothing.

A version at [[src-tauri/src/storage.rs#MAX_SUPPORTED_SCHEMA_VERSION]] still opens; anything above it fails with the machine-readable `SCHEMA_TOO_NEW:` prefix callers match on. The refusal is a hard stop — the refused database's `schema_version` must be unchanged afterwards.

##### Batched Unchanged Source Refresh

[[src-tauri/src/storage.rs#Storage#refresh_unchanged_transcript_analytics_sources]] advances many sources in one transaction without losing the per-source stale detection the per-source transactions used to provide.

It must return exactly the keys that did not update — including a row already stamped past the generation the batch was prepared against — while advancing the rest, and a mid-batch failure must roll the whole batch back rather than leaving a partially advanced root.

##### Runtime Totals Across Native Chains

`get_llm_runtime_stats` sums per native chain, so a parent and its sub-agent chains each contribute their own active interval.

Sibling sub-agents whose windows overlap in wall-clock time must not be merged into one interval, and no chain may be counted twice. The same fixture pins the `INDEXED BY idx_se_timestamp_chain` plan: the pinned query must still return these totals.

##### Workflow-Nested Sub-Agent Discovery

[[src-tauri/src/sessions.rs#SessionIndex#discover_claude_session_files_in]] recurses the whole `subagents/` subtree, so Workflow-spawned agents nested at `subagents/workflows/wf_<id>/agent-*.jsonl` are discovered alongside flat `subagents/agent-*.jsonl`.

A real projects dir with a parent `<uuid>.jsonl`, a flat sub-agent, and a workflow-nested sub-agent (leaner first record: no `cwd`/`gitBranch`/`version`) returns all three, tags only the two agents `is_subagent`, and excludes a nested `journal.jsonl`. This is the discovery stage the direct-DB runtime test cannot exercise.

##### Claude Identity Anomaly Skipping

[[src-tauri/src/transcript_analytics.rs#resolve_claude_native_identity]] skips a stray record and counts it instead of rejecting the whole source.

A record copied across a fork with its prior `sessionId` is counted into [[src-tauri/src/transcript_analytics.rs#TranscriptRecordDiagnostics]] with the ordinal of the offending line, never adopted as identity, while a later conforming record still backfills a `cwd` the first record omitted.

##### Claude Layout Hint Mismatch

A retained-layout disagreement is one anomalous fact about an otherwise usable source, so it is counted rather than discarding every row.

A parent transcript discovered under the sub-agent layout hint still resolves its parent identity and records one `layout_hint_conflicts`; agreeing parent and sub-agent layouts record none. This replaced the former hard `LayoutConflict` rejection, mirroring `model_usage.rs::accept_claude_native_source`.

##### Claude Source Without Identity

Skipping anomalies must not degrade into accepting a source that has no valid identity at all.

Records with no `sessionId`, and a sidechain record with no `agentId`, are individually skippable — but a source made only of those still fails with `MissingNativeIdentity` rather than being stamped under a guessed root.

##### Freshness Fingerprint Short-Circuits

[[src-tauri/src/transcript_analytics.rs#classify_transcript_source_freshness]] decides reparse without extracting rows, and each short-circuit must fire on exactly its own condition.

Eight cases pin the ladder: identical mtime and size skip the digest entirely (including an in-place rewrite that preserves both, which stays trusted by design); mtime drift falls through to a digest that may match or reparse; and a missing stored digest, a `failed` status, a row with no last-good identity, or a row recorded for another path all refuse the fast path. Every unchanged verdict carries the current run's generation on its owed refresh.

##### Fast Path Avoids Source Reads

The fingerprint short-circuit must return without opening the file, not merely without parsing it.

A sparse fixture larger than [[src-tauri/src/transcript_identity.rs#RETAINED_TRANSCRIPT_MAX_BYTES]] would raise `SourceTooLarge` on any read, so an unchanged verdict is proof the contents were never touched — the property that makes startup reconciliation cheap on a corpus of thousands of unchanged sources.

##### Forced Reparse Bypasses Short-Circuits

`force_full_reparse` threads the durable reingest marker through classification and must bypass both short-circuits, without ever bypassing suppression.

The fingerprint fast path and a matching content digest both yield `Changed` under force while the same fixture short-circuits without it — otherwise the flag would prove nothing. Suppressed status and a suppressed digest marker are honoured under force, because suppression is a user deletion, not a staleness verdict.

##### Forced Reparse Reads The Source

Under force, classification must actually read a source whose fingerprint matches.

An oversized fixture with a matching stored fingerprint raises `SourceTooLarge`, which only an actual read can produce — distinguishing a real bypass from a flag that merely relabels the verdict.

##### Retained Transcript Size Cap

[[src-tauri/src/transcript_identity.rs#read_stable_transcript]] enforces the 256 MiB retained cap from `metadata().len()` before allocating anything.

The guard is what keeps one pathological transcript from exhausting memory during a whole-root pass, and it must reject on apparent length rather than after a partial read.

##### Identity Comparison Excludes Cwd

[[src-tauri/src/transcript_analytics.rs#native_identity_matches]] compares only the fields that decide cross-source root membership.

A differing or absent `cwd` still matches, because `cwd` is descriptive origin and a last-good registry row can legitimately carry a different one than a fresh parse. Chain id, source session id, parent chain id, and agent id each independently break the match.

##### Commit-Time Identity Drift

Because the two reconciliation phases read at different times, a file that changes in between must not be stamped with the root resolved from its old identity.

Committing a source whose parsed identity no longer matches the inventoried one fails with `SourceIdentityDrift`, retaining last-known-good rows instead of silently reparenting them. A source that differs only by `cwd` still commits, so a moved checkout is not mistaken for drift.

##### Codex Identity Restatement And Cycles

[[src-tauri/src/transcript_identity.rs#resolve_codex_native_identity]] keeps the first child identity while tolerating consistent ancestor restatements and refusing everything else.

Thirteen cases cover root sessions, a collapsed ancestor chain, a restated child that fills a missing `cwd`, `forked_from_id` standing in for `parent_thread_id`, conflicting or dropped parents, unrelated second sessions, `A → B → A` and self-parent cycles that must terminate as conflicts rather than hang, and metadata too degenerate to yield any identity.

#### Code and Runtime Metrics

Tables for tracking active LLM session time, per-turn response latency, and cached git commit history per project.

- **session_events** — Runtime events carry `(provider, source_key, event_key, session_id, chain_id, parent_chain_id)` plus agent, timestamp, kind, UUID, and sidechain attribution. Migration 30 deduplicates owned rows by `(provider, source_key, event_key)` and source-less rows by `(provider, session_id, event_key)` through separate partial unique indexes.
- **response_times** (legacy for runtime card; still consumed by Sessions breakdown and sub-agent tree) — Per-turn latency carries the same source/root/chain lineage. Owned identity is `(provider, source_key, chain_id, timestamp)`; source-less identity substitutes `session_id` for `source_key`.

Migration 32 adds `idx_se_timestamp_chain(timestamp, provider, chain_id, is_sidechain, kind, session_id)`. The incomplete-backfill raw fallback pins this covering range index so its plan stays safe before optional manual maintenance has created `sqlite_stat1`; [[src-tauri/src/storage.rs#ensure_startup_indexes]] recreates it on every open. Completed hybrid reads instead drive from the small per-source state table and perform one correlated last-event seek through migration 31's `idx_se_provider_chain_timestamp(provider, chain_id, timestamp)`. This avoids materializing 1.5M open-turn rows on the frozen corpus while retaining the per-source finalized-rowid guard.

`get_llm_runtime_stats(range, scope)` uses raw `session_events` while runtime backfill is pending, running, or failed. Once complete, finalized turns come from `runtime_hourly`; only rows after each active source's `runtime_turn_state.finalized_through_rowid` are read through `idx_se_timestamp_chain`. Closed turns are attributed by start UTC hour and never depend on wall time. A trailing `asst_tool_use` realizes through `min(now, tool_use + 6h)`; ordinary open turns end at their last event. Distinct sessions stay provider-qualified, suppressed sources are filtered at read time, and `parent_only` uses source lineage.

Codex extraction maps user and agent text, non-empty assistant `output_text`, reasoning, function/custom calls, and call outputs into the five runtime event kinds. Developer, user, administrative, and empty message items do not become assistant runtime events. Stable native identities or source record ordinals keep source replay deterministic.

Claude records may emit multiple ordered runtime events when one content array combines thinking, text, and tool blocks. Stable per-record ordinals distinguish each event. User tool results precede same-record text and assistant tool use follows thinking/text, preserving the tool-wait transition while retaining every semantic marker.

- **git_snapshots** — Cached git history per project (project unique, commit_hash, commit_count, raw_data).

#### Metadata

Key-value configuration and schema migration version tracking.

- **settings** — Key-value config storage.
- **schema_version** — Migration version tracking (currently v37). Migration 20 truncates `response_times` and `tool_actions` (regenerable from transcripts) and sets a `subagent_reingest_pending` flag in `settings`; migration 21 adds `skill_usages` and sets `skill_usage_reingest_pending` so the next [[backend#Session Indexing]] sweep clears `index_state.json` mtimes and re-reads JSONL transcripts to backfill recognized skill-use rows. Migration 22 adds `cwd` and `hostname` columns to `skill_usages` plus the `idx_skill_usages_skill_cwd` index, and re-arms `skill_usage_reingest_pending` so historical rows refill from JSONL transcripts on the next [[backend#Session Indexing]] sweep. Migration 26 adds the `session_events` table with its unique-on-identity index and sets a `runtime_event_reingest_pending` flag so the next [[backend#Session Indexing]] sweep also clears mtimes and refills `session_events` from JSONL transcripts. Migration 27 adds the [[backend#Database#Schema#Hook Invocations]] `hook_invocations` table with one UNIQUE expression index (identity + agent_id COALESCE) plus four secondary indices (provider+timestamp, provider+session, identity+timestamp, identity+cwd), and sets a `hook_invocation_reingest_pending` flag so the same sweep replays the new attachment extractor across every Claude transcript. Migration 28 adds normalized model observations, retained-source ownership, and the singleton state that separates backfill lifecycle, root completeness, source-total publication, and bounded progress counters. Migration 29 adds the nullable indexed `derived_model_id` attribution column to `model_usage_observations`, nulls the `mtime_ns`/`content_sha256` fingerprints on active `ok` sources so their transcripts are treated as changed, and re-arms `model_backfill_state` to pending under a bumped generation with a `migration` trigger so the next startup pass genuinely re-parses and re-attributes existing evidence. Migration 30 adds [[backend#Database#Schema#Source-Owned Transcript Analytics]] and its durable rebuild marker. Migration 31 adds authoritative project rename aliases and the native-chain runtime index. Migration 32 adds the covering runtime-window index described in [[backend#Database#Schema#Code and Runtime Metrics]]. Migration 33 adds the nullable `lines_added`/`lines_removed` columns to `tool_actions` and re-arms the shared `transcript_analytics_reingest_pending` marker (the same durable flag migration 30 uses) so the next source reconciliation re-parses every transcript and backfills real counts computed before the 10KB `full_input` truncation. Migration 34 permanently drops the unused `tool_actions_legacy_v30` archive. It stamps the database at v34, so older builds refuse it and there is no downgrade path; `DROP` alone reclaims no filesystem bytes, which the separate user-triggered compact operation recovers with `VACUUM`. Migration 35 adds [[backend#Database#Schema#Retention aggregates]]. Migration 36 adds nullable source and account identity columns to `usage_snapshots`. Migration 37 adds [[backend#Database#Schema#Hourly Analytics Rollups]] without backfilling them. Existing extractor flags remain until source reconciliation replaces their shared sweep lifecycle.

[[src-tauri/src/storage.rs#MAX_SUPPORTED_SCHEMA_VERSION]] is the highest migration this build knows how to apply, and `Storage::init` compares it against the recorded version before running any migration gate. A database written by a newer build fails initialization with a `SCHEMA_TOO_NEW:`-prefixed error rather than silently skipping every unknown migration and then failing every insert against columns it cannot satisfy. Nothing is written on the way past the guard.

## Tauri IPC Commands

The Tauri commands registered in [[src-tauri/src/lib.rs]] are grouped by feature.

### Usage and Token Commands (14)

Live usage and token analytics commands back provider quota, history, breakdown, and context-savings views.

`fetch_usage_data`, `get_usage_history`, `get_snapshot_count`, `get_token_history`, `get_token_stats`, `get_provider_token_series`, `get_activity_series`, `get_token_hostnames`, `get_host_breakdown`, `get_session_breakdown`, `get_skill_breakdown`, `get_skill_project_breakdown`, `get_hook_breakdown`, `get_context_savings_analytics`.

The live-usage commands now treat utilization history as `(provider, bucket_key)` data instead of assuming a single global Claude bucket label.

`get_provider_token_series(range, buckets)` and `get_activity_series(range, buckets)` are the widget's series reads: the first returns one aligned token series per provider for the hero chart, the second the per-bucket distinct session and project counts behind the sessions and projects sparklines. Both lay their answer on the grid built by [[src-tauri/src/storage.rs#token_series_window]] — `buckets` defaults to [[src-tauri/src/storage.rs#DEFAULT_SERIES_BUCKETS]] (8) and is capped by [[src-tauri/src/storage.rs#MAX_SERIES_BUCKETS]] — so a chart and a sparkline drawn for the same range always share an x-axis. The grid ends at "now" and starts at the same [[src-tauri/src/storage.rs#range_from_timestamp]] lower bound every other range-scoped read uses; `all`, having no fixed width, starts instead at the oldest matching snapshot rather than at the epoch.

[[src-tauri/src/storage.rs#Storage#get_provider_token_series]] sums the same `token_snapshots` columns over the same `WHERE` clause as [[src-tauri/src/storage.rs#Storage#get_token_stats]] and files every matching row into exactly one bucket, so its `total_tokens` equals the headline the widget overlays on the chart — debug builds assert that identity against the headline query on the same connection and window. Two cases that would otherwise drop tokens are handled instead of discarded (constitution #1): a row whose bucket index falls outside the grid (written after the grid was computed, or with an offset that makes the string lower bound and `strftime` disagree) is clamped into the nearest bucket, and a timestamp SQLite cannot parse collapses into the first bucket rather than to `NULL`. Providers are returned as their raw snapshot strings, not parsed enums, so an unrecognized producer is still charted; series are ordered busiest first.

[[src-tauri/src/storage.rs#Storage#get_activity_series]] counts distinct `session_id` and distinct `cwd` per bucket. Those counts are distinct *within* a bucket and deliberately do not sum to a range total — a session spanning three buckets is counted in each — and snapshots with no `cwd` are left out of the project count instead of being folded into an invented "unknown" project.

Range-scoped reads share one displayed vocabulary — `1h`, `6h`, `24h`, `7d`, `30d`, and (where supported) `all`. `get_token_stats`, `get_host_breakdown`, `get_project_breakdown`, `get_session_breakdown`, `get_skill_breakdown`, `get_skill_project_breakdown`, and `get_hook_breakdown` take a `range` string instead of the earlier `days: i32`, so hour-granular selections are not rounded up. [[src-tauri/src/storage.rs#range_to_duration]] also enumerates only four internal comparison ranges: `2h`, `12h`, `2d`, and `14d`. Token, code-history, and runtime readers share that duration helper, making each internal lower bound exactly twice its displayed window without accepting arbitrary range grammar. Code-history bucket widths divide those windows at the current/prior midpoint, while comparison token rows stay intact so downsampling cannot fold evidence across it. `ModelRange` retains its separate public enum and gains no internal comparison variants.

Claude live usage comes from `https://api.anthropic.com/api/oauth/usage` via [[src-tauri/src/fetcher.rs#fetch_claude_usage]] using the local OAuth token. [[src-tauri/src/fetcher.rs#parse_buckets]] reads the flat top-level keys — `five_hour` ("5 hours") and `seven_day` ("7 days") still drive the aggregate windows and the tray indicator's short/weekly metrics. The API moved per-model weekly limits out of the flat `seven_day_sonnet`/`_opus`/`_cowork`/`_oauth_apps` keys (now returned as `null`) into a structured `limits` array; [[src-tauri/src/fetcher.rs#parse_scoped_weekly_limits]] reads each `kind: "weekly_scoped"` entry and emits a `UsageBucket` labeled by `scope.model.display_name` (e.g. `Fable` from the codenamed `omelette` slot), keyed `weekly_scoped_<model>` with `sort_order: 1`, deduped by label against the flat buckets. [[src-tauri/src/cpa/quota.rs#parse_claude_usage]] reuses that normalization, account-qualifies each key, and maps the legacy `iguana_necktie` fallback onto `weekly_scoped_fable` so mixed CPA account response shapes still aggregate into one Fable window. The `session` and `weekly_all` limits are skipped because the flat keys already cover them. Because the API returns dropped keys as `null`, their last snapshot would otherwise linger as a ghost tile in the cached live view; [[src-tauri/src/storage.rs#Storage#get_latest_usage_buckets]] prunes any bucket whose newest snapshot is more than an hour older than the provider's most recent fetch (buckets from one fetch share a timestamp, so a paused provider prunes nothing), while history and stats queries read snapshots directly and are unaffected.

Codex live usage now comes from `codex app-server` `account/rateLimits/read` instead of transcript-only scraping. The backend normalizes the returned `rateLimitsByLimitId` map into provider buckets so Quill can store both the base Codex windows and model-specific limits such as Codex Spark in the same usage tables, while preserving the legacy base Codex bucket keys for history continuity. Model-specific `limitName` values are abbreviated for display via [[src-tauri/src/fetcher.rs#abbreviate_codex_model]] (e.g. `GPT-5.3-Codex-Spark` → `5.3-Spark`) by stripping the redundant `GPT-` prefix and `-Codex` infix. The stdio helper resolves the Codex executable path, then augments the user's login-shell `PATH` with the launcher and symlink-target directories so Node-backed npm installs still start from desktop-launched Quill. It ignores unrelated app-server frames such as the `initialize` response, and only deserializes the matching request id for the rate-limit call. If the direct app-server request fails, the fetcher falls back to transcript `token_count` `rate_limits`.

MiniMax live usage comes from the coding plan API at `api.minimax.io` via [[src-tauri/src/fetcher.rs#fetch_minimax_usage]]. It reads the API key from the SQLite settings table and parses the `model_remains` array into 5-hour and weekly `UsageBucket` entries, filtering out models with zero quota.

`get_session_breakdown` now accepts optional provider and limit arguments so Codex live views can request a provider-scoped active set without being crowded out by Claude sessions.

`get_session_breakdown` remains provider-agnostic at the row level and rolls up parent plus subagent token and response rows. Storage stays retained-only and returns `observed_subagent_count: None` plus `observed_only: false`. The Tauri command lazily invalidates stale active roots, then merges the shared process-local snapshot by exact provider, normalized hostname, and root session. It advances matching retained rows from valid current-process activity and appends only active, range-matching roots with validated root cwd. Synthetic rows carry `observed_only: true`; ended, unknown, invalid, stale, filtered, or out-of-range roots never synthesize. The production SQL contains no agent-count enrichment, retention aggregate, or hook-audit scan.

The query first materializes range-, provider-, and hostname-scoped token groups, ranks them with a range-bounded indexed response-time maximum, and materializes the requested top-N before turn and project enrichment. Historical transcript and analytics storage remain available to their owning readers but do not project current subagent state.

`get_skill_breakdown` returns recognized skill-use counts from the `skill_usages` table for the widget's Skills breakdown mode. It accepts the active range string, optional Claude/Codex provider filter, all-time mode, and a capped limit; the widget always sends `all_time = false`, while other callers may still request all retained skill history. Rows sort by `total_count DESC, skill_name ASC` and include provider sub-counts plus `last_used` and a `project_count` (`COUNT(DISTINCT cwd)`) carried on each row. The widget renders one flat row per skill and has no project drilldown, so `project_count` is informational there.

`get_skill_project_breakdown` returns per-(project, hostname) counts for a single skill within the active analytics scope, retained for the per-skill project drilldown; the widget's compact Skills mode does not call it today. It accepts `skill_name`, the active range string, optional Claude/Codex provider filter, all-time mode, and a capped limit; rows sort by `total_count DESC, last_used DESC, project ASC` after applying [[src-tauri/src/storage.rs#compute_subdir_parent_map]] subdir merge so `/a/b/c` folds into `/a/b` exactly like the Projects breakdown. Rows whose transcripts lack `cwd` are preserved as a null-project bucket so expanded counts sum to the parent skill total.

`get_context_savings_analytics` returns range-scoped summary totals, timeseries buckets, grouped breakdowns, and recent append-only events for the widget's [[features#Widget Views#Context View]]. Token values are approximate `ceil(bytes / 4)` estimates, while byte counts and event counts are exact where producers can measure them.

Cacheable analytics IPC calls emit an info-level `analytics_cmd` timing record
with range, provider, cache state, and elapsed milliseconds. The record is
gated by the info log level, so it adds no elapsed-time calculation when
observability is disabled; before each command's cache map is wired it reports
`cache=miss` and retains the same stable log shape for later cache work.

### Model Analytics Commands (4)

Model analytics IPC exposes overview, paged-session, session-detail, and backfill operations through one structured, user-safe error contract.

[[src-tauri/src/lib.rs#get_model_usage_overview]] validates the fixed time range and optional provider before reading the one-snapshot Models-page overview described in [[backend#Database#Schema#Model Analytics Evidence]] off the async command thread.

[[src-tauri/src/lib.rs#get_model_sessions]] validates the fixed range and exact provider-qualified opaque model ID, preserves the storage-owned 20-row null default, and clamps signed numeric limits to 1–100 before platform-independent conversion. Malformed, foreign, or stale opaque cursors return `invalid_cursor` without exposing cursor diagnostics.

[[src-tauri/src/lib.rs#get_session_model_history]] validates provider and range before loading one provider-owned session. Missing in-range retained evidence returns `not_found`, distinct from storage failure.

[[src-tauri/src/lib.rs#retry_model_history_backfill]] reserves scheduling before advancing the durable retry generation, returns current state when that retained pass is already scheduled or running, and treats an unowned persisted `running` row as interrupted work. A live-source owner can release the shared process permit before the pending pass starts.

Storage and blocking-task failures stay in local logs. All four commands return only the bounded serialized model analytics error envelope, and model IDs use shared opaque Unicode validation without a catalog or version allowlist.

### Indicator Commands (2)

`get_indicator_primary_provider` and `set_indicator_primary_provider` keep one backend-owned indicator model shared across the tray title, tray summary rows, and the integrations menu.

`set_indicator_primary_provider` persists the configured provider in the settings table, recomputes the resolved indicator state from the shared usage cache or fallback rows, and emits `indicator-updated` so the tray summary and integrations menu stay synchronized without a second polling path.

### Project and Session Management (3)

`get_project_tokens`, `get_session_stats`, `get_project_breakdown`.

The manage-data deletion and rename commands (`delete_host_data`, `delete_session_data`, `delete_project_data`, `rename_project`) and their storage helpers were removed; no shipped surface invoked them.

### Integration Commands (15)

Commands for detecting providers and running install/uninstall flows, plus per-provider and global feature toggles.

Provider setup state is persisted through the settings table using key `integration.providers.v1` to survive app restarts. Three global feature flags — `context_preservation.enabled` (default false), `feature.activity_tracking.enabled` (default true), and `feature.context_telemetry.enabled` (default true, gated on context preservation) — drive which optional Quill assets get deployed into Claude Code and Codex.

The `confirm_enable_provider` command accepts an optional `api_key` parameter used by service-only providers like MiniMax and reads the global `IntegrationFeatures` from storage so newly-enabled providers inherit the current feature set automatically. `get_context_preservation_status` also reports whether historical context-savings events exist which the widget Context view uses to distinguish an unconfigured store from an empty range; the view itself is always registered in the switcher.

`rescan_integrations` drops the cached login-shell PATH (see [[src-tauri/src/config.rs#refresh_shell_path]]) and re-runs detection so users who edit their shell config or install a CLI mid-session can pick it up without restarting Quill. Failed CLI detections persist the candidate paths inspected on `ProviderStatus.lastDetectionAttempts` so the integrations menu can show why a provider is "N/A" despite being installed.

`set_minimax_api_key` updates a stored MiniMax API key in place (no disable/re-enable round-trip) and emits `integrations-updated`.

`set_cpa_connection`, `clear_cpa_connection`, and `get_cpa_connection_status` own the CPA service-source lifecycle. Connect accepts a loopback URL plus management key and returns typed smoke verdicts without returning or logging the key. Status exposes only URL/configured state. Clear removes both connection keys, every `usage.cpa.*` key, CPA raw/hourly rows, and invalidates the usage-cache epoch; all mutations run under the shared integration guard.

`get_integration_features` returns the resolved `IntegrationFeatures` struct. `set_activity_tracking_enabled`, `set_context_telemetry_enabled`, and `set_brevity_enabled` each save their flag, reinstall every currently-enabled provider via [[src-tauri/src/integrations/manager.rs#apply_features_to_enabled_providers]] (which also re-syncs brevity blocks via `sync_brevity_blocks`), and emit `integration-features-updated`. The existing `set_context_preservation_enabled` follows the same path so all four feature toggles share one sync function.

`get_provider_statuses`, `rescan_integrations`, `confirm_enable_provider`, `confirm_disable_provider`, `get_context_preservation_status`, `set_context_preservation_enabled`, `set_minimax_api_key`, `set_cpa_connection`, `clear_cpa_connection`, `get_cpa_connection_status`, `get_runtime_settings`, `set_runtime_settings`, `get_integration_features`, `set_activity_tracking_enabled`, `set_context_telemetry_enabled`, and `set_brevity_enabled`.

At startup, [[src-tauri/src/integrations/manager.rs]] verifies enabled, detected Claude and Codex providers against the stored context-preservation setting. Codex verification resolves the install-state home, parses semantic feature/MCP values, and requires `hooks/list` to return every exact Quill handler enabled and trusted; substring matches no longer count as installed. Missing or stale assets trigger an idempotent reinstall, while repair failures leave the provider enabled but persist `last_error` and an error setup state.

### Runtime Settings Commands (2)

Single IPC pair backing the [[features#Settings Window]]'s Performance, General (always-on-top), and Learning (rule watcher) tabs.

`get_runtime_settings` returns the resolved `RuntimeSettings` struct with `live_usage.enabled`, `live_usage.interval_seconds`, `rule_watcher.enabled`, and `always_on_top` clamped to safe ranges (live: 60–600s). `set_runtime_settings` and the tray checkitem share [[src-tauri/src/lib.rs#apply_runtime_settings]]: a nonblocking gate rejects concurrent writers, then the admitted transition derives prior topmost state from persisted settings, requires the main window for a changed topmost value, submits `WebviewWindow::set_always_on_top` before any save, commits all runtime keys through [[src-tauri/src/storage.rs#Storage#set_settings_atomically]], synchronizes the checkitem, and emits `runtime-settings-updated`. The gate cannot block the event-loop tray handler while an async IPC worker waits for menu work marshalled to that thread; a rejected tray event restores its auto-toggled checkmark from the currently committed setting. The native setter reports request submission, not compositor acknowledgement; an API error aborts the transition, while X11/Wayland policy can still delay or ignore a successfully submitted request. Any reported persistence, menu, or emit failure compensates the native state, persisted values, checkmark, and crash-reporting state already changed, returning the primary failure plus rollback failures. The tray uses its captured `CheckMenuItem::is_checked()` value as desired state because muda toggles it before dispatching `MenuEvent`; it never inverts the potentially lagging window getter. [[src-tauri/src/lib.rs#seed_widget_always_on_top]] runs once at startup: while the `widget_ui_v1` marker is absent it seeds `always_on_top` to `true` for the widget, but only when no value is stored, so a user who deliberately turned it off keeps that choice.

### Retention policy commands

The read/write pair over [[backend#Backend#Database#Retention policy primitive]], kept out of the wholesale-saved `RuntimeSettings` because a retention window is consented to one value at a time, not saved alongside unrelated toggles.

[[src-tauri/src/lib.rs#get_retention_policy]] returns the three `settings` rows
as one [[src-tauri/src/retention.rs#RetentionPolicy]]. It is cheap settings
reads only — no scan, no quiesce lease, no `spawn_blocking` — so the settings
surface can render the configured window and the last-run audit record without
contending with an in-flight maintenance operation.

[[src-tauri/src/lib.rs#set_retention_policy]] writes `retention.window_days` and
returns the refreshed policy. It never touches `retention.watermark` and never
deletes a row. [[src-tauri/src/lib.rs#apply_retention_policy]] is the testable
core: it validates **before** writing, so only
[[src-tauri/src/retention.rs#RETENTION_WINDOW_PRESETS]] and `None` are accepted
and every other value errors with the stored window untouched. This validation
*is* the 30-day floor. The floor is what makes `get_code_stats`,
`get_code_stats_history` and `get_llm_runtime_stats` provably unaffected by
retention — `range_to_duration` caps every range-based reader at 30 days — so a
`7` slipping through the command boundary would silently revoke that guarantee
rather than merely prune more aggressively. The primitive re-checks the same
list on write and on read; the command boundary is the outermost of the three.

#### Retention Policy Command Test Specs

The command layer owns one spec: that the preset list is a closed set at the
boundary the frontend actually calls, since that is where a user-supplied value
first enters.

##### Preset Rejection

`set_retention_policy` accepts exactly 30, 90, 180, 365 and `None`, and rejects
every other value with an error that leaves `retention.window_days` unchanged.

The rejected set covers 7, 1, 0, -90 and 45, the 29/31 and 366 boundaries
either side of the preset list, and both `i64` extremes.

Asserting the *unchanged* half matters as much as the rejection: a validator
that errors after a partial write would leave the database configured with a
window nobody consented to, which is exactly the failure the floor exists to
prevent.

### Retention preview command

[[src-tauri/src/lib.rs#preview_retention]] is the consent gate: it counts exactly what a prune would remove and mints the `cutoff` token the destructive run demands, which is what makes a run backend-side unreachable without a preview.

The guarantee is structural rather than procedural. `run_retention_maintenance`
accepts only a `confirmed_cutoff`, and this command is the only thing that
produces one, so a UI that forgot to ask cannot prune — the backend refuses on
its own. [[src-tauri/src/retention.rs#derive_retention_cutoff]] is called
**once**, here, and the value travels to the run verbatim; a cutoff re-derived
inside the run would sit later than the one the user approved and delete rows
this preview never counted.

The counts are exact, not estimated, because the preview runs the *same*
[[src-tauri/src/retention_engine.rs#scan_doomed_rows]] pass the run does, under
the same ingest quiesce lease — taken through
[[src-tauri/src/lib.rs#try_begin_ingest_quiesce]], so a preview fired while
another maintenance operation holds it returns the shared busy skip rather than
freezing — and the same `spawn_blocking` treatment as `compact_database`. Consenting to this payload is therefore consenting to the
set the run deletes, which is the whole point of the counting being shared
rather than approximated by a cheaper query.

#### Why the counting phase is where progress matters most

The preview is *nothing but* the counting phase, so a bar pinned at zero is not a cosmetic flaw here — it is the entire visible behaviour of the command.

Percentages go out through [[src-tauri/src/lib.rs#emit_retention_maintenance_progress]]
under [[src-tauri/src/lib.rs#RETENTION_PHASE_COUNTING_ROWS]] — the run's emitter
and the run's phase vocabulary, not a third event — so the Settings UI keeps one
listener pair for "previewing" and "running".
[[src-tauri/src/lib.rs#build_retention_preview]] emits one tick before the scan
opens so the phase is on screen from the first frame, and the scan's own
wall-clock heartbeat plus its per-table completion nudges carry it to 100.

#### Telling two zeroes apart

A preview that counts nothing is a skip, and *which* skip is a different sentence to the user: an empty database has no history, while a populated one simply has none old enough yet.

[[src-tauri/src/retention_engine.rs#count_owned_rows]] is what separates them. It
also closes the partition the scan opens — owned rows are exactly the doomed set,
plus the pre-cutoff rows the conformance guard kept, plus everything at or after
the cutoff — so `everything_older` falls out by subtraction instead of a third
full pass over both tables. `everything_older` is the flag that drives the blunt
"this removes all of it" confirmation copy rather than the ordinary
"older than N days" copy.

The skip vocabulary is [[src-tauri/src/lib.rs#RETENTION_DISABLED_REASON]] (no
window configured — the one case with no cutoff to return at all),
[[src-tauri/src/lib.rs#RETENTION_FRESH_INSTALL_REASON]], the run's own
[[src-tauri/src/retention_engine.rs#RETENTION_NOTHING_OLDER_REASON]], and the
shared [[src-tauri/src/lib.rs#RETENTION_BUSY_REASON]]. Operational
failures a user can act on — a database that will not open, a file that cannot be
stat'd — are structured skips too, matching the run; a SQL failure mid-scan is
not, because it means the database is in a state neither the command nor the user
can reason about.

#### Consent to a capability, not to a row count

[[src-tauri/src/lib.rs#RETENTION_AFFECTED_SURFACES]] rides on the ready payload because "delete 689,441 rows" is not something anybody has an intuition for.

The three surfaces named — session drilldowns, subagent trees, batch session code
stats — are the only readers a window at the 30-day floor can starve, since
`range_to_duration` caps every range-based reader at 30 days. The list ships from
the backend rather than living in the frontend so the copy and the cutoff that
justifies it always arrive together. A skip carries an empty list: nothing is
being removed, so nothing is being lost.

#### Retention Preview Command Test Specs

These specs pin the property the consent gate exists for — that the number a user
approves is the number the run removes — and the three edge states where the
honest answer is "nothing to do" or "all of it".

##### Preview Accuracy

The preview's exact counts must equal the run's deleted counts on a quiesced
fixture with no interleaved writes, with the run driven by the preview's *own*
cutoff rather than a re-derived one.

The cutoff must land on the fixture's 90-day boundary, the per-table counts must
match the plan's arithmetic for both doomed and non-conforming rows, and the
counting phase must open at 0, pass through each table's half of the bar, and
close at 100 without ever going backwards.

##### Fresh Install Previews Nothing

A database with no source-owned rows at all must preview zero and skip with the
fresh-install reason, never with "nothing older than the cutoff" — a user with an
empty database is not being told their history is too young.

The cutoff such a preview still mints must drive a run that also skips, deleting
nothing.

##### Nothing Older Previews Nothing

A populated database whose rows are all newer than the cutoff must preview zero
and skip with the nothing-older reason, and the run driven by that cutoff must
skip as well.

The distinction from the fresh-install case is the assertion: the same zero
counts must carry a different, correct explanation.

##### Everything Older Is Reported As Total

A cutoff newer than every source-owned row must set `everything_older`, report
the whole owned corpus per table, and leave the run free to proceed to a
completed delete of exactly those rows.

This is the case that needs the blunt confirmation copy, so a preview that
reported it as an ordinary partial prune would understate what the user is
agreeing to.

### Composite retention command

[[src-tauri/src/lib.rs#run_retention_maintenance]] is the only destructive retention entry point: one quiesce lease held across scan, delete-phase preflight, chunked deletes, VACUUM, audit write and cache invalidation, driven by a cutoff the user already confirmed.

[[src-tauri/src/lib.rs#execute_retention_maintenance]] is the testable core; the
command is a thin `run_blocking` wrapper that supplies `Utc::now()`, the two
[[backend#Backend#Database#Retention maintenance events]] emitters, and the
spike's chunk size. The SQL is synchronous, so running it on the async runtime
would stall every other IPC for the whole lease — the same reason
`compact_database` uses `run_blocking`.

#### The confirmation is the consent

The two parameters bind the run to a preview: a `confirmed_cutoff` and the
`confirmed_window_days` it was derived under.

A parameterless run would recompute `now - window` at invocation, which deletes
strictly *more* than was previewed — including rows that aged past the boundary
while the confirm step was on screen. Small in seconds, unbounded if the dialog
sits open overnight, and a consent violation in every case, because the user
approved a specific boundary date. The confirmed token is therefore used
verbatim for the scan, the deletes, the watermark advance and the audit record,
and is never re-derived.

[[src-tauri/src/lib.rs#retention_confirmation_is_fresh]] refuses on either of
two independent staleness conditions: the stored `retention.window_days` no
longer equals `confirmed_window_days` (the user changed the preset after
previewing), or the confirmation trails a freshly derived cutoff by more than
[[src-tauri/src/lib.rs#RETENTION_STALE_PREVIEW_TOLERANCE_MS]] — one Counting
phase from the [[backend#Backend#Database#Retention timing spike]], the point at
which a preview costs more to trust than to redo. A cutoff that cannot be parsed
is refused the same way, because a token nothing can compare is a token nothing
may delete on. The skip reason is the machine token `stale_preview` rather than
a sentence: it is the one skip whose remedy is an action the UI takes
(re-preview) instead of copy it renders.

This also closes a hole UI discipline alone cannot. The only source of a valid
`confirmed_cutoff` is a preview, so no caller — a stray `invoke`, a future
automation, a bug in the confirm flow — can prune without having produced the
numbers the user was shown.

#### Order of operations

Validation happens **before** the lease is taken, and the lease is held until
the function returns.

Validating first means a refused confirmation never holds the gate for a moment,
and it is what makes "a refusal mutates nothing" provable rather than hoped for:
the only database work a refused run performs is the policy read. Holding the
lease to the end means the VACUUM that turns freed pages into freed bytes runs
inside the same quiesce window the deletes did, so no ingest write lands between
the two halves of one maintenance operation.

Between those two points the sequence is fixed: scan → optional atomic JSONL
archive → delete preflight → chunked deletes → **close the maintenance
connection** → VACUUM preflight → VACUUM → audit rewrite → cache clear. The
connection close is not incidental —
[[backend#Backend#Database#Retention delete engine]] owns two `TEMP TABLE`s and
`vacuum_database` will not rebuild the file underneath another connection
holding schema-visible temp state.

#### Two statuses, not one

`compaction_status` is reported separately from `status`, and the phase is only
announced on the path that actually reaches VACUUM.

S2 requires that a failed VACUUM preflight still reports the rows that were
removed, so `status: "completed"` with `compaction_status: "skipped"` and
`bytes_after == bytes_before` has to stay expressible — "rows removed, bytes not
yet reclaimed" is a legitimate outcome, not a failure. A `partial` run does not
attempt compaction at all and says so; a run that deleted nothing has nothing to
reclaim and says that instead. Because the delete engine writes its record with
`bytes_after == bytes_before` (true at the time — deletes free no filesystem
bytes), a VACUUM that reclaims bytes rewrites the record with the figure the user
is actually shown. A failed rewrite downgrades to a warning: the durable record
is already correct about what was deleted.

`Err` is reserved for faults that leave the outcome indeterminate — a chunk-level
SQL failure, a stalled chunk loop, an audit write that did not land. The three
[[src-tauri/src/retention_engine.rs#RetentionDeleteError]] variants that fail
before any chunk transaction opens (`MalformedCutoff`, `Connection`,
`WatermarkAdvance`) become structured skips instead, because the database is
provably untouched and a maintenance operation that reports "error" when it
simply had nothing to do teaches people to ignore it.

#### Composite Retention Command Test Specs

These specs pin what the composite layer alone owns: the consent binding, the
lease's two-sided contract, and the fact that retention is invoked and never
scheduled.

##### Deferred Ingest Survives The Retention Lease

An ingest write fired into an active retention window must stay pending for the
whole window and land once the lease releases — never dropped, never rejected as
a hard error.

The retention lease is acquired differently from the compaction one, so the
guarantee users depend on has to be re-proved against it rather than inherited.

##### A Held Lease Is A Skip Not A Wait

With the gate already held, the composite command must return promptly with the
structured busy skip and mutate nothing — no rows, no watermark, no audit record,
not even a progress tick.

The failure this prevents is a second maintenance command blocking unboundedly on
`RwLock::write()` with no feedback. Both leased retention commands acquire
through the same refusable call, so the assertion covers the preview as well as
the run; the policy reads are asserted in the same window because they hold no
lease at all and must stay responsive while a prune runs.

##### Stale Confirmations Are Refused

A changed preset, a confirmation aged past the tolerance, and a cutoff that
cannot be parsed must each yield the `stale_preview` skip with zero rows deleted
and the watermark unmoved.

All three mean the same thing — the token no longer binds the user's consent —
and all three have the same cheap remedy, so all three must refuse identically
rather than one of them silently proceeding.

##### The Confirmed Cutoff Is Used Verbatim

A fresh confirmation must delete exactly the rows older than the confirmed token
and record that exact string as the cutoff and the watermark, with live rows
untouched, the caches invalidated, and the phase vocabulary emitted in order.

Running it from an instant *after* the one that derives the token is what makes
the assertion sharp: a run that re-derived instead of honouring the confirmation
would record a different string and still look plausible.

##### Every Skip Path Leaves The Database Alone

Retention disabled, nothing older than the cutoff, and a refused delete-phase
preflight must each report a structured skip, delete no row, and leave the
watermark exactly where it was.

Advancing the watermark on a path that deleted nothing would suppress inserts the
user never consented to lose — the one failure mode this design must not have.

##### A Partial Run Does Not Compact

A run stopped between chunks must report `partial` with an `error_reason`, count
only what committed, keep the watermark advanced, and neither attempt nor
announce compaction.

Announcing the compaction phase and then not compacting is a worse lie than
skipping it, so the phase emission and the compaction decision are asserted
together.

##### Retention Schedules Nothing

Nothing in the retention path may register a timer, interval or detached task:
retention runs only from an explicit command invocation.

The guard is structural — it reads the bracketed retention command surface plus
the two retention modules, matching call shapes rather than words so a doc
comment cannot trip it — because a scheduler added later is the most likely way
this non-goal gets lost, and no behavioural test would notice.

### Learning Commands (14)

Commands for managing the behavioral learning pipeline settings, rules, and observations.
Read and trigger commands accept an optional provider filter so the UI can request Claude-only, Codex-only, or combined learning views.

`get_learning_settings`, `set_learning_settings`, `get_learning_capability`, `get_learned_rules`, `delete_learned_rule`, `promote_learned_rule`, `submit_rule_feedback`, `get_learning_runs`, `trigger_analysis`, `get_observation_count`, `get_unanalyzed_observation_count`, `get_top_tools`, `get_observation_sparkline`, `read_rule_content`.

State-changing learning commands are authorized (feature 005 US2 — H-4 / FR-011, see `specs/005-learning-system-hardening/contracts/ipc-and-feedback.md`). At startup the backend mints an ephemeral per-process capability token (`OsRng`, held only in Tauri managed state, never persisted). `get_learning_capability` returns it ONLY to the window whose label is `learning`. A single reusable guard runs first on every mutating command — constant-time token compare via the `subtle` crate plus a `learning`-window-label assertion — and is applied to `delete_learned_rule`, `promote_learned_rule`, and `submit_rule_feedback` (each gains a `token` arg). All three `submit_rule_feedback` values (`accept`/`reject`/`bad`) are guarded — `bad` writes a durable tombstone and changes active state, while `accept`/`reject` carry the same trust as promote/delete per the contract (feature 005 US3 — FR-029). The counterfactual evaluation harness and its command surface (`run_rule_evaluation`, `record_reviewer_override`, `rollback_rule`, `reactivate_rule`) were removed; promotion no longer consults evaluation results. Read commands (`get_learned_rules`, `read_rule_content`, `get_learning_runs`, …) stay unauthenticated. The HTTP `POST /api/v1/learning/rules` ingest keeps its bearer auth and is clamped to `lifecycle='candidate'` — its payload carries no lifecycle field and `store_learned_rule` is structurally incapable of producing `awaiting_review`/`active`.

`get_top_tools` intentionally reads exact raw-observation windows instead of reusing `observation_summaries`, because summary rows are keyed by cleanup period rather than original event timestamps. The backend relies on the covering observation indexes above to keep that exact-window query responsive.

### Code and Response Stats (5)

`get_code_stats`, `get_code_stats_history`, `get_batch_session_code_stats`, `get_llm_runtime_stats`, `get_session_subagent_tree`.

`get_batch_session_code_stats` fans out one SQL branch per `(provider, session_id)` pair with `UNION ALL` so SQLite can use the `tool_actions` provider/session index instead of falling back to a broad category scan across the entire code-change corpus.

`get_llm_runtime_stats(range, scope)` accepts an optional `scope: "all" | "parent_only"` argument and uses the completed hybrid runtime path described in [[backend#Database#Schema#Code and Runtime Metrics]], falling back to raw evidence until backfill completes. `None` or `"all"` includes every active source; `"parent_only"` filters source lineage. The IPC return shape (`LlmRuntimeStats { total_runtime_secs, turn_count, session_count, avg_per_turn_secs, sparkline }`) is unchanged from migration 25's contract.

`get_session_subagent_tree(provider, session_id) -> Vec<SubagentNode>` is retained for sub-agent drilldowns; no shipped surface calls it today, because the widget's compact Sessions rows do not expand. Implementation in [[src-tauri/src/storage.rs#Storage#get_session_subagent_tree]] returns one node per `agent_id` for the requested `(provider, session_id)`, carrying `parent_agent_id` (null at depth 1; populated when Claude later spawns depth-2+ sub-agents and rebuilt at query time from `parent_uuid` chains), `first_seen`/`last_active`, `turn_count`, the input/output/cache/total token breakdown, `tool_call_count`, and a reserved `label: Option<String>` (always `None` today).

### Memory Optimizer Commands (15)

Commands for managing memory files, optimization runs, and suggestion approval workflows.
Most read and trigger commands accept an optional provider filter for Claude, Codex, or combined views.

`get_memory_files`, `trigger_memory_optimization`, `get_optimization_suggestions`, `approve_suggestion`, `deny_suggestion`, `undeny_suggestion`, `undo_suggestion`, `approve_suggestion_group`, `deny_suggestion_group`, `get_optimization_runs`, `get_known_projects`, `add_custom_project`, `remove_custom_project`, `delete_memory_file`, `delete_project_memories`.

### Session Indexing Commands (4)

`search_sessions`, `get_session_context`, `get_search_facets`, and `sync_search_index` all operate on a unified Claude-plus-Codex index. Search and context requests include provider identity so session collisions do not bleed across providers.

`sync_search_index` runs an mtime-based incremental sweep — not a wipe-and-rebuild — so a true rebuild requires deleting the on-disk index dir while the app is closed (or bumping `SCHEMA_VERSION` in [[src-tauri/src/sessions.rs]]).

### Restart Commands (5)

`request_restart`, `cancel_restart`, `get_restart_status`, `install_restart_hooks`, `check_restart_hooks_installed`.

Restart commands expose a shared provider-aware row model across Claude and Codex. Hook install/check commands accept an optional provider parameter so restart setup can be applied per provider.

Claude setup resolves its persisted `ClaudePaths`, runs under the integration mutation guard, and commits settings, restart ownership, hook assets, and shell RC blocks as one restart-specific transaction. `check_restart_hooks_installed` parses exact handler command/args/timeout tuples and verifies current script/block contents; malformed state or configuration reports not installed instead of accepting a filename substring.

### UI Commands (4)

`hide_window`, `quit_app`, `install_app_update`, `get_release_notes`.

[[src-tauri/src/lib.rs#install_app_update]] re-checks the configured updater from Rust, downloads and installs the release, logs the resolved relaunch binary, then schedules a detached relaunch via [[src-tauri/src/lib.rs#spawn_delayed_relaunch]] and exits the primary so the titlebar update button shares the backend-owned install-and-relaunch boundary with the tray updater. The detached relaunch is required because `tauri-plugin-single-instance` would otherwise treat the new process as a duplicate launch (see [[architecture#Architecture#Single Instance]]).

[[src-tauri/src/lib.rs#get_release_notes]] proxies the public GitHub releases API for `sharaf-nassar/quill` via [[src-tauri/src/releases.rs#fetch_release_notes]], drops drafts and prereleases, and returns a normalized `ReleaseNote` list (tag, name, body, html url, published_at) that the [[frontend#Frontend#Components]] release-notes window paginates with Previous/Next. The command takes an optional `limit` (clamped to 1-100, default 30) so the frontend can request a small newest-first window without exposing GitHub pagination details. Unauthenticated requests are used because the repository is public; rate-limit and HTTP errors are surfaced as `Result::Err` strings rather than swallowed.

### Integration Commands (12)

Integration IPC exposes provider detection, manual rescan, provider enablement, the global context-preservation toggle, the global brevity toggle, and the in-place MiniMax API-key update.

CPA adds a masked read command plus guarded connect/disconnect commands. The key is accepted only on connect and never returned; disconnect purges the complete settings and snapshot footprint before the usage-cache epoch advances.

`get_provider_statuses`, `confirm_enable_provider`, `confirm_disable_provider`, and `get_context_preservation_status` expose provider state and the context-preservation setting. `set_context_preservation_enabled` installs or removes local context-preservation assets for currently enabled Claude and Codex providers without deleting historical context data.

`get_provider_statuses` returns the last saved provider statuses from storage rather than re-running detection. Fresh detection happens once at startup via the background `startup_refresh` task, which saves results and emits `integrations-updated`. This avoids redundant subprocess calls and eliminates the visible "Checking integrations..." loading state on the main window.

In `QUILL_DEMO_MODE=1`, startup refresh and manual rescan still detect providers
and persist status in the isolated database, but skip interrupted-deployment
recovery and enabled-provider repair so runtime fixtures never mutate real
provider configuration.

`rescan_integrations` is the explicit retry path: it calls [[src-tauri/src/integrations/manager.rs#force_rescan]] (which clears the cached login-shell PATH and dynamic-prefix cache via [[src-tauri/src/config.rs#refresh_shell_path]] and reruns `startup_refresh`), then invalidates and re-warms the usage cache so a previously-N/A provider that just flipped to detected is reflected in the tray indicator without waiting for the next polling cycle. Used by the integrations menu's "Rescan PATH" button when the user has just installed a CLI or edited shell config.

Detection runs via `--version` checks for CLI providers through the shared [[src-tauri/src/config.rs#detect_provider_cli]] helper, which both `claude_setup::detect` and `codex::detect` delegate to so a single fix to PATH augmentation, error handling, or timeouts covers both providers. The shared resolver in [[src-tauri/src/config.rs#resolve_command_path]] layers a login-shell `command -v` lookup with a static fallback list (bun, cargo, deno, volta, npm-global, n, asdf, mise, nodenv, Nix profile, yarn classic, `~/.claude/local/`, Linuxbrew, Homebrew, MacPorts, snap) and dynamic `npm config get prefix` / `bun pm bin -g` / `yarn global bin` queries — covering installs whose dirs only appear in interactive shell config (`~/.zshrc`) which `zsh -lc` does not source. Dynamic-prefix outputs are validated against a trusted-roots allow-list before being added to the candidate list so a malicious `npm config set prefix /tmp/evil` cannot trick Quill into executing an attacker-controlled binary. Failed detections record every path inspected on `ProviderStatus.lastDetectionAttempts` with `$HOME` redacted to `~/...` (and the field skipped from JSON when empty) so the integrations menu can show a per-row diagnostic tooltip without leaking the local username. Service-only providers like MiniMax skip CLI detection and use API key presence instead. Implementation lives in [[src-tauri/src/integrations/mod.rs]], [[src-tauri/src/claude_setup.rs]], [[src-tauri/src/integrations/codex.rs]], and [[src-tauri/src/config.rs]].

## Event System

The backend pushes real-time updates to the frontend via Tauri's emit system.

| Event | Source | Payload | Trigger |
|-------|--------|---------|---------|
| `tokens-updated` | server.rs | `()` | Token snapshot stored |
| `context-savings-updated` | server.rs | `()` | Context savings events stored |
| `learning-log` | learning.rs | `{run_id, message}` | Real-time analysis progress |
| `learning-updated` | lib.rs | `()` | Rules changed |
| `provider-status-updated` | integrations | `Vec<ProviderStatus>` | Startup provider detection refresh |
| `restart-status-changed` | restart.rs | `RestartStatus` | Restart phase change |
| `integrations-updated` | integrations/manager.rs | `ProviderStatus[]` | Startup refresh or provider enable/disable completed |
| `context-preservation-updated` | integrations/manager.rs | `ContextPreservationStatus` | Global context-preservation toggle changed |
| `integration-features-updated` | integrations/manager.rs | `IntegrationFeatures` | Activity tracking or context telemetry toggle changed |
| `runtime-settings-updated` | lib.rs | `RuntimeSettings` | Live-usage / rule-watcher / always-on-top toggle changed |
| `indicator-updated` | lib.rs | `StatusIndicatorState` | Shared usage refresh or primary-provider change recomputed indicator state |
| `transcript-analytics-updated` | lib.rs | `()` | Transcript analytics committed, renamed, or deleted |
| `memory-optimizer-log` | memory_optimizer.rs | `{message}` | Optimization run progress |
| `memory-optimizer-updated` | memory_optimizer.rs | `{run_id, status}` | Run completed |
| `memory-files-updated` | memory_optimizer.rs | `{project_path}` | Memory files changed |

Indicator state payloads use the explicit status vocabulary `ready`, `degraded`, or `unavailable` so the frontend can treat healthy state distinctly from warning and empty-state cases without legacy `ok` handling.

## Session Indexing

[[src-tauri/src/sessions.rs]] provides full-text search over Claude Code and Codex transcripts using Tantivy, with provider-safe identity for indexing, search hits, context lookup, and reindex cleanup.

### Index Schema

The Tantivy index stores provider, identity, content, and enrichment fields for shared session search.

Fields include provider, message_id, session_id, content, role, project, host, timestamp, git_branch, tools_used, files_modified, code_changes, commands_run, tool_details, and a stored display text field. Provider/project/host are faceted for filters. Stored at `~/.local/share/com.quilltoolkit.app/session-index/`.

### Indexing Strategy

Session Search triggers an incremental mtime scan of `~/.claude/projects/` and `~/.codex/sessions/**` before loading facets, while hook-driven notify/message ingestion keeps the index fresh during app runtime.

A complete provider-root scan also removes Tantivy documents and mtime state for vanished supported transcripts; incomplete roots never authorize deletion.

When a transcript is reprocessed, Quill coalesces repeated search `notify` requests per session and applies each Tantivy rewrite under one writer lock with a single commit. One retained-source coordinator separately tracks model and transcript completion under canonical `(provider, source_key)`; transcript replacement still spans all five tables in one transaction. The mtime sweep deletes existing session docs unconditionally before reinserting, even on first sight of a file, so hook-driven indexing cannot stack duplicate copies.

Skill usage is derived by [[src-tauri/src/sessions.rs#extract_skill_accesses_from_tool_action]], which recognizes read-like loads of a `SKILL.md` file and derives the skill name from that file's parent directory. Retained rows are owned and replayed by canonical source, and the extractor does not infer skills from assistant prose, available-skill lists, or skill-file maintenance edits. Flattened `/sessions/messages` payloads contain no tool-action detail and emit no skill rows.

The shared Claude candidate walker descends through the complete `<projectSlug>/<session-uuid>/subagents/**` subtree in addition to flat parent transcripts. Its permissive search view preserves original paths, project labels, and `is_subagent`; its strict retained view canonicalizes containment and source identity. Both admit only nested `agent-*.jsonl` transcripts, excluding Workflow journals. Claude extraction reads `isSidechain`, `agentId`, and `parentUuid`; Codex preserves the first child `session_meta.id` and resolves `parent_thread_id`, falling back to `forked_from_id`.

The HTTP API also accepts provider-tagged notify and direct message ingestion. Local Claude full-transcript sync is Stop-scoped, while direct message ingestion still appends atomically for incremental remote updates. BM25 scoring plus snippet generation power the shared search UI with provider filters and badges.

[[src-tauri/src/sessions.rs#validate_retained_notify_source]] validates one `notify` path against only its configured provider root, canonical containment, and supported layout without walking transcript history. Quill admits a canonical source to model and transcript reconciliation before session-keyed search coalescing; a resolvable path that fails the stricter retained-source policy still coalesces for search only, preserving the indexing contract. Direct message payloads append Tantivy documents and atomically store source-less runtime rows plus recorded live origin through [[src-tauri/src/storage.rs#Storage#store_live_session_analytics]].

### Search Scoring

Query parsing applies per-field BM25 boosts so concrete artifacts outrank noisy metadata.

The default-search field weights are: `files_modified` (4.0), `code_changes` (2.5), `commands_run` (2.5), `tool_details` (1.5), `content` (1.0), and `tools_used` (0.5). Without these boosts, equal weighting plus BM25 length-normalization let short fields like `tools_used` (where every session contains tokens like `Edit` and `Bash`) dominate ranking. The derived `display_text` field is kept in the parser at boost 0.1 only so Tantivy's `SnippetGenerator` — which filters terms by field — can highlight matches against it; it is a superset of `content + code_changes + commands_run + tool_details` and would otherwise double-count every term. Query-parser errors from `parse_query_lenient` are logged at debug level instead of being silently discarded.

## Claude Code Inference Client

[[src-tauri/src/cc_client.rs]] is the single inference surface for the app. Every LLM call (learning streams + synthesis, memory optimizer, prose compression) spawns the `claude` CLI in headless one-shot mode rather than making a direct HTTP request to Anthropic.

Public surface: [[src-tauri/src/cc_client.rs#invoke_typed]] for schema-validated structured output and [[src-tauri/src/cc_client.rs#invoke_text]] for free-form prose. Model routing is pinned at the CLI boundary to `claude-sonnet-4-6` for all work — pattern extraction, learning synthesis (single-model since feature 005 US5 T060/H-7; no rolling `sonnet` alias, stable cost attribution), and prose work. `--json-schema` is unreliable (the CLI does not enforce it), so typed calls do not use it. `invoke_typed` instead embeds the JSON Schema in the prompt, grants the headless agent a `Write`-only tool sandboxed to a per-call temp dir, and has it write the result to `out.json`; Quill reads that file and `serde_json::from_str::<T>` is the sole validation (missing/invalid → `SchemaValidationFailed`, no app-side retry). `invoke_text` is unchanged (free-form, total tool isolation).

The `claude` binary is located via [[src-tauri/src/config.rs#resolve_command_path]] — the same cached, login-shell-aware resolver used for provider CLI detection — so it picks up Anthropic's `claude migrate-installer` target and auto-refreshes when the user triggers a PATH rescan. Each invocation runs `claude -p --output-format json --model <alias> --append-system-prompt <preamble> --tools "" --disable-slash-commands --no-session-persistence --setting-sources "" --exclude-dynamic-system-prompt-sections` and pipes the prompt body on stdin, joined with `wait_with_output` in a single future so a large prompt cannot deadlock against the child's stdout. The subprocess is isolated from the user's interactive Claude Code configuration (their hooks, slash commands, plugins, CLAUDE.md auto-discovery, and session history are all suppressed) and runs with `CLAUDE_CODE_*`, `ANTHROPIC_*`, and `NODE_OPTIONS` scrubbed from the inherited environment.

No app-side retry, no `Retry-After` interpretation, no rate-limit backoff. Each invocation has a 300-second hang-detector timeout (via `tokio::time::timeout` + `kill_on_drop`). Errors are categorized into eight stable variants — `ClaudeCodeMissing`, `ClaudeCodeTooOld`, `NotSignedIn`, `RateLimited`, `SchemaValidationFailed`, `TimedOut`, `Spawn`, `BadEnvelope` — each producing a user-facing message that names the cause and the actionable remediation. When `BadEnvelope` fires on a successful exit (status=0, stdout unparseable), the error string is enriched with the exit status and the first 1024 chars of stderr so silent-exit failures (e.g. a sandboxed launcher catching a denied path and `process.exit(0)`-ing without writing the envelope) stay diagnosable from the `learning_runs.error` / `optimization_runs.error` column. See `specs/003-cc-inference-migration/contracts/cc-client.md` for the full contract.

On top of the in-process flag isolation (defense in depth, kept verbatim), the spawned `claude` is wrapped with the best-available OS-level confinement because it processes untrusted captured content. [[src-tauri/src/cc_client.rs#apply_sandbox]] runs as the last step of `build_command`, rewrapping the fully-formed command. Linux is a three-tier chain — **Landlock** (primary; in-process kernel LSM, no user namespaces, no AppArmor entanglement) → **Bwrap** (subprocess fallback for kernels without Landlock or hosts where bwrap is still permitted) → **None** (unconfined, honestly recorded, actionable diagnostic emitted). macOS uses `sandbox-exec` with a deny-by-default SBPL profile (reads scoped to system/runtime prefixes + the resolved claude/node tree, **no** `$HOME`/`~/.claude`/`~/.config`/project access; writes confined to the per-call temp dir); Windows relies on the existing `kill_on_drop` Job Object association (documented best-effort). The Linux primary tier applies a Landlock ruleset built by [[src-tauri/src/cc_client.rs#build_ruleset]] from a [[src-tauri/src/cc_client.rs#LandlockPolicy]] (ABI v3 declared with `CompatLevel::BestEffort` so older Landlock-capable kernels degrade access rights cleanly) via a forked-child pre-spawn hook on the `tokio::process::Command`'s underlying `std::process::Command::pre_exec` — the hook runs `prctl(PR_SET_NO_NEW_PRIVS, 1, …)` then `RulesetCreated::restrict_self()` in the child between `fork` and `execve` so Quill itself stays unrestricted; the ruleset grants RO `path_beneath` rights to `{/usr, /bin, /sbin, /lib, /lib32, /lib64, /etc, /opt, /nix-if-present, /proc, /sys, /dev, /run/systemd/resolve, /run/dbus, claude_install_root, ~/.claude.json, ~/.claude}` and RW rights to `{per-call TempDir, /dev/null}`, with absent optional paths silently skipped (mirrors bwrap's `--ro-bind-try`). The host pseudo-filesystems `/proc`, `/sys`, `/dev` are in the RO set because Landlock has no mount namespace (unlike bwrap's `--proc`/`--dev`/`--tmpfs` which inject fresh ones) — denying them makes the launcher's Bun runtime SIGILL at startup on `readlink(/proc/self/exe)` / `open(/dev/urandom)` / `open(/proc/cpuinfo)`; the trade-off vs bwrap is that `/proc/N/*` exposes other PIDs' cmdline/environ to the subprocess. The `~/.claude.json` + `~/.claude/` RO entries deviate from spec 007's original "no `$HOME` / no `~/.claude`" design — required because claude 2.1.152's Bun launcher reads its config + cached OAuth credentials from those paths during startup and, on EACCES (vs. ENOENT), silently `process.exit(0)`s with empty stdout/stderr (no actionable error). Read-only `path_beneath` lets the launcher authenticate without giving the subprocess write access to session history, hooks, plugins, or the credentials file; the rest of `$HOME`, `~/.config`, and project trees stay denied. The `/run/systemd/resolve` + `/run/dbus` RO entries are required for the spawned child's DNS resolution when Quill spawns from a Tokio runtime (which is always the case in production — Quill is a Tauri/Tokio app): `/etc/resolv.conf` is a symlink to `/run/systemd/resolve/stub-resolv.conf` on systemd-resolved hosts, and the Tokio-context resolver follows it (a std-context resolver happens to succeed without `/run` access — see R-H). Both `/run` paths are tiny transient state, contain no user data, and `path_beneath_rules` silently skips them on hosts without systemd-resolved. See `specs/007-landlock-inference-sandbox/research.md` R-G + R-H for the bisection evidence. [[src-tauri/src/cc_client.rs#build_command]] also exports `TMPDIR=<per-call dir>` and `NODE_COMPILE_CACHE=<per-call dir>` on the typed-call path so the launcher's transient writes route into the already-allowed RW dir instead of `/tmp` (no-op under bwrap, which gives a private tmpfs `/tmp`, and under `None`). The Bwrap fallback's argument construction is byte-for-byte the same as before feature 007 (deny-by-default filesystem, no `$HOME`/`~/.claude`/`~/.config`/project access, a single RW bind of the per-call temp dir); only its *position* in the chain moved from primary to first fallback. The previous `unshare`-based `ProcessOnly` tier introduced by feature 006-A is **retired** — it required the same `CLONE_NEWUSER` capability AppArmor blocks on Ubuntu 24.04+, so it was theatrical on exactly the hosts that broke bwrap, with no FS-confinement value either way. When the chain falls all the way through to `None`, a process-wide one-shot diagnostic ([[src-tauri/src/cc_client.rs#emit_no_confinement_diagnostic]], guarded by `OnceLock<()>`) is emitted to both `log::error!` (visible in the `tauri dev` terminal) and the per-call log channel that lands in `learning_runs.logs` (visible in run-history detail) — two templates: a **generic FR-014** message when neither mechanism is available at detection, and an **AppArmor-specific FR-015** message when bwrap was attempted and failed because of Ubuntu 23.10+'s default `kernel.apparmor_restrict_unprivileged_userns=1` policy (detected by [[src-tauri/src/cc_client.rs#classify_bwrap_failure]] returning [[src-tauri/src/cc_client.rs#BwrapBrokenCause]]`::AppArmorRestrictUserns` after substring-matching bwrap stderr against `"setting up uid map: Permission denied"` or `"loopback: Failed RTM_NEWADDR: Operation not permitted"`); a process-wide `OnceLock<BwrapBrokenCause>` latch prevents re-spawning the same known-broken bwrap on subsequent calls in the same Quill process. Network is deliberately preserved on every branch (no net namespace / `network-outbound` allowed, no Landlock network rules) — the CLI makes the model API call itself. Helper binaries are still resolved via a `std::env::split_paths` PATH scan plus absolute fallbacks; the one new approved dependency is `landlock` 0.4.4 (Apache-2.0/MIT, kernel-feature author's crate, Linux-only target-cfg). Confinement **never fails closed**: if Landlock build/probe errors, the chain falls through to bwrap; if bwrap is absent or latched-broken, the flag-isolated command runs unchanged and inference continues; the reduced state is recorded. See `specs/005-learning-system-hardening/research.md` R-7.6, `specs/006-learning-hardening-followups/research.md` R-A, `specs/007-landlock-inference-sandbox/research.md` R-A..R-F, `specs/007-landlock-inference-sandbox/contracts/landlock-sandbox.md` C-A..C-E, and FR-005/SC-013.

The structured `--output-format json` envelope returned by every call carries per-call metadata (input/output tokens, cache stats, model id, durations, cost, stop reason, permission denials) that is captured into [[src-tauri/src/cc_client.rs#InferenceCallMetadata]] and persisted on the parent run record's `inference_metadata` JSON column for both `learning_runs` and `optimization_runs`. The record also carries a `sandbox` field — one of the closed write vocabulary `{"landlock", "bwrap", "sandbox-exec", "job-object", "none"}` ([[src-tauri/src/cc_client.rs#SandboxKind]]) — recording the applied OS confinement for every call on both the success and `failed_metadata` paths so SC-013 (confinement state recorded for 100% of analysis runs on every platform) is verifiable. The tag is honest about the boundary: [[src-tauri/src/cc_client.rs#sandbox_tag_is_fs_confined]] (single source of truth, keyed on the stable tag) is `true` for `landlock`/`bwrap`/`sandbox-exec` (real deny-by-default filesystem confinement) and `false` for `job-object`/`none`; the classifier stays **tolerant of any legacy tag** including the retired `"process-only"` and pre-feature-006 `"unshare"` (both → `false`) and any unknown future tag (→ `false`), so historical rows decode forever without migration (feature 007 contract C-D). Feature 006 Follow-up A's operator-disclosure plumbing is preserved unchanged: [[src-tauri/src/storage.rs#decode_inference_metadata]] projects a derived `confinement` (`{ sandbox, fs_confined }`) onto each `RunInferenceCall` and an `all_fs_confined` rollup onto `RunInferenceSummary`, and [[src/components/learning/RunHistory.tsx]] renders a distinct amber marker plus the remediation hint for any run that recorded a not-FS-confined call (FS-confined and legacy/no-inference runs render unchanged).

[[src-tauri/src/fetcher.rs]] is the only remaining consumer of the Claude Code OAuth credential in the codebase. It powers the [[features#Live Usage View]] band by polling `api.anthropic.com/api/oauth/usage` and was intentionally not migrated as part of feature 003 (see `specs/003-cc-inference-migration/spec.md` FR-015). A 401 from that endpoint is treated as a stale access token (a muted "Paused" state), so the only logged-out warning path runs [[src-tauri/src/config.rs#claude_logged_in]], which spawns `claude auth status --json` UNCONFINED to read the `loggedIn` boolean without touching the credential store — see [[data-flow#Usage Bucket Fetching]] step 8d.

## Git Analysis

[[src-tauri/src/git_analysis.rs]] (343 lines) extracts commit patterns for the [[features#Learning System]].

Collects commit messages, file hotspots (change frequency), co-change patterns (files changed together), and directory structure. Excludes merge commits (>20 files) and minified code. Results cached by project + HEAD commit hash, invalidated on HEAD change. Compressed to 4,500 bytes for LLM context. Commit lines are prefixed with the git `%h` short-hash and the compressed block leads with a `[SNAPSHOT HEAD <hash>]` key (feature 005 US3 T040, H-1) so Stream B can emit resolvable `kind="commit"` evidence refs that [[src-tauri/src/storage.rs#Storage#resolve_evidence_refs]] verifies via `git cat-file` or the `git_snapshots` cache; redaction still runs before compression so the cache stays secret-free.

Every git-derived text field (commit subjects, hotspots, diff stats, folder structure, and per-commit co-change file lists) is passed through [[src-tauri/src/redaction.rs#redact]] before `compress_git_data` truncates and before the result is written to the `git_snapshots.raw_data` cache. The cached value and the prompt value are therefore both redacted, so a cache hit cannot re-leak a secret.

## Concurrency

The backend uses Tokio for async operations with specific patterns:

- `tokio::task::block_in_place()` for sync DB/file operations within async context
- `tokio::spawn()` and `tauri::async_runtime::spawn()` for background tasks
- `std::sync::Mutex<T>` / `std::sync::RwLock<T>` for synchronization, including invalidatable single-writer caches (e.g. the login-shell PATH cache in [[src-tauri/src/config.rs]]); the `parking_lot` dependency was removed
- `Arc<T>` for shared ownership across task boundaries
- `OnceLock<T>` for one-time initialization of globals (STORAGE, HTTP_CLIENT) — used only for caches that never need to be invalidated; invalidatable caches use `std::sync::Mutex<Option<T>>` or `std::sync::RwLock<Option<T>>` instead

## Platform-Specific Code

Conditional compilation targets for Unix signal handling, macOS Keychain, and cross-platform paths.

- `#[cfg(unix)]` — Process signal handling (SIGUSR1 for restart), nix crate for signal/process, `setsid` + env-var handshake for update-driven relaunch (see [[architecture#Architecture#Single Instance]])
- `#[cfg(target_os = "macos")]` — Keychain integration for credential reading
- Cross-platform path resolution via `dirs` crate

## Error Handling

All IPC commands return `Result<T, String>` for frontend-friendly errors. Internal functions use `.map_err()` chains with context. No panics in public APIs.

`log::error!()` / `log::warn!()` for debugging. Graceful degradation throughout.

Live-usage IPC carries one structured exception to the plain-string contract: `UsageData.provider_errors` is `Vec<UsageProviderError>`, where each entry pairs the provider with a typed [[src-tauri/src/models.rs#ProviderErrorKind]] discriminator (`Network`, `Config`, `Auth`, `Server`, `Paused`, or `Stale`) alongside the human-readable message. A 429 maps to consequence-oriented `Stale` after arming the rate-limit cooldown; no cause-oriented `RateLimit` payload exists. The LIMITS-header sync control ([[src/components/widget/LimitsSection.tsx#LimitsSection]]) collapses `Network`, `Stale`, and `Paused` entries into one degraded state, while provider rows retain `Config`, `Auth`, and `Server` detail. The flow that drives this — rate-limit and transport cooldowns, exponential half-jitter backoff, and the on-success counter clear — is documented under [[data-flow#Data Flow#Usage Bucket Fetching]] steps 8a–8c.

## Data Paths

Key filesystem locations used by the backend for storage, config, and caching.

| Path | Platform | Purpose |
|------|----------|---------|
| `~/.local/share/com.quilltoolkit.app/` | Linux | DB, search index, auth secret |
| `~/Library/Application Support/com.quilltoolkit.app/` | macOS | DB, search index, auth secret |
| `~/.config/quill/` | All | Deployed hooks, MCP server, scripts |
| `~/.claude/` | All | Claude Code config, credentials |
| `~/.cache/quill/` | All | Instance state files, restart flags |

### Demo-mode path override

All call-sites that previously hard-coded the data dir, learned-rules dir, or Claude projects dir now route through [[src-tauri/src/data_paths.rs#resolve_data_dir_with_default]], [[src-tauri/src/data_paths.rs#resolve_rules_dir_with_default]], and [[src-tauri/src/data_paths.rs#resolve_claude_projects_dir_with_default]] so a maintainer can launch a sandboxed Quill instance against dummy data without touching their personal state.

The override is gated by an explicit opt-in: `QUILL_DEMO_MODE=1` is required, and `QUILL_DATA_DIR` / `QUILL_RULES_DIR` / `QUILL_CLAUDE_PROJECTS_DIR` are otherwise ignored even when set. With opt-in active and a per-variable override set, paths are canonicalized via `std::fs::canonicalize` (creating the directory first if missing); a canonicalize failure exits the process with code 2 so the demo never silently falls back to the real data dir under a confused launcher. If a Claude or Codex session-root override is missing in demo mode, its resolver returns an empty temporary placeholder instead of indexing the production transcript root. A one-time `[quill-demo] data_dir=… rules_dir=…` banner prints to stderr on first resolver call so a demo run is impossible to confuse with a real one. With `QUILL_DEMO_MODE` unset, behavior is byte-identical to the production path table above.

Used by the marketing-site screenshot-capture workflow (`scripts/run_quill_demo.sh`); see [[infrastructure#Scripts#Demo Launcher]].
