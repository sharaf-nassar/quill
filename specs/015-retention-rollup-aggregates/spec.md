# Spec: retention-rollup-aggregates

## Problem Statement

Feature 014 ships retention as an outright delete: the user picks an age window,
previews, confirms, and Quill removes source-owned pre-cutoff rows from
`tool_actions` and `session_events`, then compacts. The rows are gone. The
consumer-side treatment (S4, `RetentionBanner` + `src/utils/retention.ts`) is
deliberately cheap — it marks pre-cutoff ranges and relabels `all` as "all
retained" — so the product is *honest* about the hole but the hole is total.

**Corrected during review:** an earlier draft of this paragraph claimed the loss
shows up as a missing long-horizon trend chart. It does not. `RangeType` is
`1h | 24h | 7d | 30d`, `range_to_duration` has no `all` arm, and the 30-day
retention preset floor exists precisely so range readers can never reach a pruned
row. The loss is confined to **session-scoped** surfaces, and it is four scalars
wide — see Spec Review CQ1.

Rollup aggregates preserve the *shape* of the pruned window at a fraction of the
bytes: before the delete engine removes a range, summarize it into surviving
daily (or per-session) aggregate rows, and teach the readers that today assume
raw rows to splice those aggregates in below the cutoff.

This is a deferred Non-Goal of 014, filed as bead `quill-upx` under the
now-closed epic `quill-nm2`. The MVP decision to ship deletion without rollup is
**settled and not re-litigated here**; this feature adds a layer on top of a
shipped, working delete path.

Who: any Quill user who enables retention and still wants long-horizon trend
data. Why now: retention shipped, so the first users to enable it are about to
create the first permanent holes.

## Goals

1. **Pre-cutoff activity survives deletion in aggregate form.** After a
   retention run with rollup enabled, a reader asking for a range that extends
   below the watermark gets real numbers for that range — coarser than raw rows,
   but not an em dash and not a zero.
2. **The aggregate write is durable-consistent with the delete.** There is no
   outcome in which rows are deleted (or suppressed by the watermark) and their
   aggregate was never written. Crash-mid-run must not silently manufacture a
   permanent hole.
3. **Idempotent across repeated runs.** Pruning to 180 days, then later to 90,
   must not double-count the 180–∞ window. Re-running the same cutoff is a
   no-op.
4. **Bounded, measured cost.** The rollup pass has a published budget measured
   on the same frozen fixture as the delete engine, and the combined
   rollup+delete+VACUUM lease stays within a duration the UI can honestly state.
5. **Readers degrade to aggregates instead of to nothing.** The surfaces that
   today carry a `RetentionBanner` gain a third state: `retained` (raw),
   `aggregated` (rolled up, coarser), `pruned` (genuinely gone).
6. **Coexists with the sibling export/archive feature** (`quill-uhg`), which
   wants its own pre-delete hook in the same engine. One extension point, two
   consumers.
7. **Opt-in and reversible-ish.** Rollup is a setting, not a forced behavior. A
   user who does not want a new table does not get one.

## Non-Goals

- **Not a re-litigation of 014.** Deletion, the consent preview, the watermark,
  the quiesce lease, the conformance guard, and the S4 degradation treatment
  ship as-is and are consumed, not redesigned.
- **Not retention for other tables.** `model_usage_observations`,
  `observations`, `usage_snapshots`, `token_snapshots` stay out of scope
  (separate bead `quill-i8b`).
- **Not export or archive of raw rows.** JSONL/Parquet sidecars and `*_archive`
  tables are `quill-uhg`. This feature writes *summaries*, not row copies. The
  two must share a hook; they do not share a format.
- **Not reconstruction of pruned rows.** An aggregate is lossy by design. No
  drilldown from an aggregate back to individual tool calls.
- **Not aggregate retention.** Aggregates are small enough to keep forever in
  the MVP of this feature. A retention policy *for the aggregate table* is a
  future problem.
- **Not automatic backfill of already-pruned windows.** A user who already ran
  014 retention lost those rows; this feature cannot resurrect them. It applies
  to runs from its ship date forward.
- **Not a change to the live-row exclusion.** `source_key IS NULL` rows are
  never deleted and therefore never rolled up.
- **Not scheduled/automatic retention.** Still strictly manual, still opt-in.

## User Stories

### 1. Prune with rollup enabled

**As a** Quill user with two years of history and a 7.5 GB database,
**I want** retention to summarize what it deletes,
**so that** I reclaim the bytes without losing my long-term trend lines.

Acceptance Criteria:
- Given rollup is enabled and a 90-day window, when the retention run completes,
  then every pre-cutoff day that had ≥1 source-owned conforming `tool_actions`
  or `session_events` row has a corresponding aggregate row.
- Aggregate rows carry, at minimum: date bucket, session identity, provider,
  source ownership, per-table row counts, and the token/duration/code-change
  measures the degrading readers need.
- The whole-file byte reduction after rollup+delete+VACUUM is within [OQ: N]% of
  the reduction the same run would have achieved without rollup.
- The audit record (`retention.last_run`) reports aggregate rows written
  alongside rows deleted.

### 2. Crash mid-run does not create a silent hole

**As a** user whose laptop lost power during a prune,
**I want** the run to be either fully summarized or safely resumable,
**so that** I never end up with rows suppressed and no aggregate to show for
them.

Acceptance Criteria:
- Given the process is killed between the watermark advance and the completion of
  the rollup pass, when Quill restarts, then either (a) the aggregates for every
  window the watermark now suppresses are present, or (b) the run is reported as
  `partial` with an `error_reason` naming the un-aggregated window, and the UI
  states it.
- No ordering exists in which the watermark advances past a window whose
  aggregate was neither written nor reported missing.
- [OQ: does this force the rollup write *before* the watermark advance, and if
  so what is the cost of that ordering?]

### 3. Readers show coarse history below the cutoff

**As a** user looking at the sessions breakdown for "all time",
**I want** pre-cutoff periods to show aggregated figures rather than em dashes,
**so that** the chart reads as a real history with a resolution change, not a
cliff.

Acceptance Criteria:
- `get_session_breakdown`, `get_session_subagent_tree` and
  `get_batch_session_code_stats` (today's three degrading readers) return
  aggregate-backed values for pre-cutoff spans when aggregates exist.
- The span classifier gains an `aggregated` state; `retentionSpanFor` and
  `markPrunedRange` return it, and `PRUNED_PLACEHOLDER` is used only where no
  aggregate exists.
- The banner copy distinguishes "summarized" from "removed".
- Readers provably unable to reach a pruned row (`get_code_stats`,
  `get_code_stats_history`, `get_llm_runtime_stats` — capped at 30 days by
  `range_to_duration` against a 30-day preset floor) are **unchanged** and must
  not gain aggregate handling.
- [OQ: 014 Q5 asserts "five read paths". Today's degradation surface names three
  readers. Identify the exact five before planning, or correct the count.]

### 4. Opt out and stay opted out

**As a** user who wants the space back and does not care about history,
**I want** rollup to be off unless I turn it on,
**so that** my database gains no table and my prune gains no runtime.

Acceptance Criteria:
- Default is rollup disabled; a database that never enables it never runs the
  migration [OQ: is a conditional migration acceptable, or does the table always
  get created empty?].
- Disabling rollup after use leaves existing aggregates readable and stops new
  ones being written.

### 5. Repeated and tightening prunes stay correct

**As a** user who prunes to 180 days now and 90 days next year,
**I want** the second run to summarize only the newly doomed window,
**so that** my aggregate totals are not inflated.

Acceptance Criteria:
- Re-running an identical cutoff writes zero new aggregate rows.
- Tightening the cutoff aggregates only the `[old_watermark, new_cutoff)` band.
  (Cutoffs are timestamps; tightening 180d → 90d moves the cutoff *later*, and
  the delete predicate is strict `timestamp < ?1`. An earlier draft inverted both
  the order and the brackets.)
- Aggregate rows are keyed such that a repeat write is an upsert, not a
  duplicate.

## Constraints

- **Origin and settled decisions.** Deferred Non-Goal from
  `specs/014-retention-pruning` (Clarifications Q5), origin bead `quill-upx`
  under closed epic `quill-nm2`. The MVP's choice to ship the cheap degradation
  treatment (S4: mark/truncate pre-cutoff ranges, relabel `all` as "all
  retained") is not reopened.
- **This feature breaks 014's "no schema" property.** 014's data model is
  deliberately three `settings` rows — `retention.window_days`,
  `retention.watermark`, `retention.last_run` — with **no table, no migration,
  and no schema-version bump**, which is exactly what makes an older build's
  reopen a non-event. A rollup table is the first retention migration and
  forfeits that property. This is the single largest cost of the feature and
  must be argued explicitly, not assumed.
- **The delete engine is the integration point.**
  `src-tauri/src/retention_engine.rs#run_retention_delete_phase` opens its own
  maintenance connection, materializes doomed rowids, preflights free disk,
  advances `retention.watermark` to the cutoff **before the first chunk
  transaction opens** (not before it commits — the distinction is load-bearing),
  then deletes in chunks with a WAL checkpoint after each commit. The rollup pass
  must slot into this sequence without breaking the watermark's forward-only,
  crash-durable contract.
- **The two connections cannot share a transaction.** The watermark rides the
  **primary** connection (`Storage::advance_retention_watermark` takes
  `self.conn.lock()`); the doomed rowids live in `temp.retention_doomed_*` on the
  **maintenance** connection and are invisible to the primary. `drain_target`'s
  own doc comment records why they cannot be merged: WAL permits exactly one
  writer, so a primary-connection write issued while the maintenance connection
  holds an `IMMEDIATE` transaction deadlocks the run against itself until
  `busy_timeout` expires. Any design that "puts the rollup and the watermark
  advance in one transaction" is unimplementable.
- **Watermark semantics are load-bearing.** `max(existing, cutoff)` applied with
  read and write in one transaction; it only moves forward; a committed chunk
  leaves it permanently advanced through a `partial` outcome, a skipped VACUUM,
  or a failed one. A run that skips at the delete-phase preflight must **not**
  advance it.
- **Conformance guard.** The engine refuses to delete rows failing
  `length(timestamp) = 24 AND timestamp LIKE '%Z'`. Those rows survive below the
  cutoff, so rollup must not count them as deleted — and the degradation copy
  already says "may be incomplete" for this reason.
- **Live rows are excluded.** `source_key IS NULL` rows (from
  `store_live_session_analytics` and `persist_remote_session_analytics`) are
  excluded from every DELETE and from the watermark filter. Rollup inherits that
  exclusion.
- **Lease pressure.** Rollup runs inside the same
  `try_begin_ingest_quiesce` lease as the delete and the VACUUM. That lease is
  already long and is currently unbounded and uncancellable. Adding a full scan
  of the doomed range before deleting it makes a known weak point worse.
  **Corrected during review:** `retention_spike.rs` never runs a VACUUM at all.
  The ~82 s figure came from `vacuum_spike.rs` on a 7.45 GB synthetic corpus, and
  a real 7.54 GB production copy measured 168 s. 014's published 40,598 ms
  retention budget is scan+delete only, on the 1.29 GB fixture — three corpora,
  not one.
- **Budgets come from the frozen fixture.**
  `src-tauri/src/retention_fixture.rs#build_retention_fixture` is the one
  synthetic corpus every retention test and
  `src-tauri/src/bin/retention_spike.rs` run against, so acceptance numbers and
  budget numbers cannot drift onto separate corpora. Any rollup budget must be
  measured there.
- **Sibling feature shares the hook.** `quill-uhg` (export or archive rows
  before pruning) wants a pre-delete extension point in the same engine. Spec
  the hook so both can attach; do not design a rollup-only special case.
- **Cache invalidation is the run's last step.**
  `src-tauri/src/lib.rs#invalidate_analytics_after_retention` drains five
  in-process analytics caches and emits `transcript-analytics-updated`.
  **Corrected during review:** that drain is defensive and is a no-op for this
  feature — none of the five caches (`model_analytics`, `model_usage_overview`,
  `model_history`, `bucket_stats`, `context_savings_analytics`) serves any of the
  three degrading readers, and neither retention target is a `CacheTable`. The
  real invalidation path is the `transcript-analytics-updated` event and its
  frontend consumers.
- **Accepted limitation, inherited.** `subagent_count` UNIONs
  `token_snapshots ∪ response_times ∪ tool_actions` and only the last is pruned;
  014 accepted the mixed-horizon result. Rollup may change that arithmetic and
  must say whether it fixes, preserves, or worsens it.
- **No constitution.** This repo has no `constitution.md`, so there are no
  recorded engineering principles to check this spec against.
- **Design system.** Any new UI states follow `DESIGN.md` — chrome-grey for
  retention treatment; green/amber/red are reserved for the severity meter.

## Open Questions

1. **Ordering vs. the watermark.** Must the rollup write commit before the
   watermark advances? That is the only ordering with no silent-hole window, but
   it puts a full scan of the doomed range ahead of the point of no return and
   lengthens the lease before any bytes are reclaimed. Alternative: a resumable
   rollup cursor that lets the watermark advance first.
2. **Grain: daily, per-session, or per-(session, day)?** Per-session preserves
   drilldown shape; daily is far smaller; per-(session, day) is the honest
   product of the two readers' needs and the largest. What does
   `get_session_breakdown` actually need to render a pre-cutoff row?
3. ~~**How many read paths, exactly?**~~ **Resolved during review — three.**
   Six readers touch the two target tables; three are provably unreachable below
   the cutoff by the 30-day cap (`get_code_stats`, `get_code_stats_history`,
   `get_llm_runtime_stats`). The three that degrade are `get_batch_session_code_stats`,
   `get_session_breakdown` (`subagent_count` only) and `get_session_subagent_tree`.
   014's "five" counted readers touching `tool_actions`; its Constraints table
   also carries a phantom `session_events → get_session_subagent_tree` row that
   does not exist in the code.
4. **Conditional vs. unconditional migration.** Creating the table only when the
   user enables rollup preserves 014's "older build reopen is a non-event"
   property for everyone else, but conditional migrations are their own hazard.
5. **What measures does an aggregate carry?** Row counts alone are cheap but
   probably useless; tokens, durations, and code-change line counts are what the
   readers actually display. Each added measure is a column and a cost.
6. **Does rollup extend or replace the S4 treatment?** If aggregates exist for a
   window, does the banner disappear, change wording, or stay?
7. **Lease budget.** What is the measured rollup cost on the frozen fixture, and
   does the combined lease need an abort affordance before this ships? (The
   unbounded/uncancellable lease was already flagged as a weak point during 014.)
8. **Interaction with `quill-uhg`.** If a user enables both export and rollup,
   does the doomed range get scanned once or twice?
9. **Non-conforming timestamps.** Rows the guard refuses to delete survive below
   the cutoff. Are they rolled up too (double-counting them against their own
   surviving raw rows), or excluded (making aggregates disagree with raw counts
   in that band)?
10. **Aggregate correctness testing.** How is "the aggregate matches what the
    raw rows said" verified — a fixture-based golden comparison before/after a
    simulated prune?

## Spec Review

Six parallel review passes (requirements, gaps, ambiguity, feasibility, scope,
stakeholders) against the shipped 014 code, the frozen fixture, the timing-spike
artifacts, and a read-only query against a live 7.5 GB `usage.db`. Findings that
appeared in three or more passes are marked **[cross-dimension]** and carry the
highest confidence. Six factual errors in the first draft were corrected in place
above; the review that found them is recorded here.

### Critical Questions (answer before planning)

1. **Should this feature be built in this shape at all, given that the entire
   loss surface is four scalars and a much cheaper design recovers most of
   it?** — *flagged by: scope, feasibility, requirements* **[cross-dimension]**

   The complete set of values that disappear below the cutoff is: per-session
   `lines_added`/`lines_removed`; `tool_call_count` per `(session, agent_id)`;
   the `tool_actions` arm of the `agent_id` union; and `parent_agent_id`
   resolution. Everything else those surfaces render — tokens, `turn_count`,
   `first_seen`, `last_active`, `project`, `has_subagents` — comes from
   `token_snapshots`/`response_times`, which 014 keeps at full history. A pruned
   session does not vanish; it loses four numbers.

   Measured on the live corpus, **category-selective retention** recovers most of
   that for one predicate in two places (the doomed-rowid scan and the
   insert-time watermark filter):

   | category | rows | payload bytes |
   |---|---:|---:|
   | `code_change` | 22,510 | 54,474,593 |
   | `command` | 213,070 | 668,398,409 |
   | `tool_detail` | 161,092 | 515,024,132 |

   Exempting `category = 'code_change'` preserves all three code-stats readers
   **losslessly, with full drilldown**, for 5.7% of rows and ~5.2% of payload —
   on a 90-day window, about 9.4k rows / 23 MB against a 2.15 GB table. No table,
   no migration, no reader branch, no `aggregated` span state, no crash-ordering
   redesign, no golden-correctness test, no shared hook. Counterweight: the
   exempted slice grows unbounded (~190 MB/yr at current rates) and needs a
   secondary longer window eventually.

   After that carve-out the residual value of this feature is **one integer** —
   `tool_call_count` per subagent. Does that justify a new table plus a
   re-ordering of a shipped crash-durability invariant? Answer explicitly.

2. **What is the grain, given that one required reader cannot be aggregated at
   any grain?** — *flagged by: feasibility, gaps, stakeholders, ambiguity, scope*
   **[cross-dimension]**

   `get_session_subagent_tree` reads `tool_actions` at three sites: the
   `agent_id` universe, `tool_call_count`, and — critically —
   `parent_agent_id` resolution via a `message_id` join. The third is a graph
   edge recovered by row identity, not a measure; **no aggregate at any grain
   preserves it**, so the tree would come back as a flat list of orphans.
   `subagent_count` is separately non-additive — it is `COUNT(DISTINCT agent_id)`
   over a three-table union, so per-day buckets double-count any agent active on
   more than one day, and two of the three unioned tables are never pruned (the
   aggregate must be *unioned in*, not summed).

   So the grain menu in OQ2 (daily / per-session / per-(session, day)) is not
   answerable as posed: the floor is per-`(session, agent_id)`, nothing needs a
   date bucket, and the parent edge needs a separate decision — preserve it some
   other way, or drop that reader from scope.

3. **Where does the rollup write land relative to the watermark advance, the
   disk preflight, and the chunk loop — and what is its failure taxonomy?** —
   *flagged by: ambiguity, feasibility, requirements, gaps* **[cross-dimension]**

   Goal 2's atomicity is unimplementable (see the corrected Constraint: primary
   vs. maintenance connection, WAL single-writer deadlock). The only viable
   ordering is aggregate-commits-then-watermark-commits, which satisfies Goal 2
   **only if idempotent upsert is a hard prerequisite rather than a separate
   story** — make US5 a dependency of US2, not a peer.

   Still unanswered: does the rollup run before or after `preflight_delete_phase`
   (before = writes with no disk check; after = the preflight is knowingly
   under-priced, since `RETENTION_WAL_BYTES_PER_ROW = 788.7` and
   `RETENTION_TEMP_BYTES_PER_DOOMED_ROW = 11.05` price only the chunk WAL and the
   temp tables)? Does a failed rollup abort the delete, or proceed? Is it a
   `skipped`, a `partial`, or a hard `RetentionDeleteError`? Does the watermark
   stay put, matching the preflight-skip rule?

   US2's crash criterion (b) is unachievable as written: `finish()` writes
   `retention.last_run` **in process**, so a power loss never reaches it.
   Delivering (b) needs an in-flight marker written before the watermark advance
   plus a startup reconciliation step, neither of which exists anywhere in the
   codebase. A resumable cursor is also harder than it looks — `drain_target`
   chunks by **rowid**, and rowid order is not timestamp order, so a crash can
   only report "some rowids are done", which is not the "window" the criterion
   promises.

4. **Should `session_events` be cut from scope entirely?** — *flagged by: scope,
   feasibility*

   Its only production SQL reader is `get_llm_runtime_stats`, which is 30-day
   capped and therefore provably cannot reach a pruned row.
   `get_session_subagent_tree` does **not** read it — 014's Constraints consumer
   table carries a phantom row that does not exist in the code, and that phantom
   is where the "five read paths" count came from. Yet `session_events` is the
   *larger* half of the prune (1.6 M rows / ~2.57 GB), so including it doubles
   the write path, the fixture assertions, and the lease cost for a table nothing
   can read below the cutoff.

5. **What is the migration posture — and is a migration needed at all?** —
   *flagged by: gaps, stakeholders, scope, feasibility* **[cross-dimension]**

   The Constraints section calls the lost "no schema" property the single largest
   cost. Both halves of that framing are wrong, in opposite directions:

   - **Worse than stated if versioned.** Recording schema 35 does not degrade
     anything — `MAX_SUPPORTED_SCHEMA_VERSION = 34` and the `SCHEMA_TOO_NEW`
     guard make every older build **refuse to open the database at all**. There
     is no downgrade path and no backup/export affordance anywhere in the app.
   - **Possibly free if unversioned.** `ensure_startup_indexes` is an in-repo
     precedent for schema DDL with no version bump; its own comment says it
     "needs no schema bump and no `SCHEMA_TOO_NEW` lockout". A
     `CREATE TABLE IF NOT EXISTS` on that path costs no v35 and no lockout — an
     older build ignores an unknown table exactly as it ignores unknown
     `settings` keys.

   Residual cost of the unversioned route: a table drifting outside the migration
   ledger, so every future migration touching it must handle "table may not
   exist". Note the knock-on — if the table is free for non-adopters, the entire
   justification for US4 (opt out) and OQ4 dissolves, and rollup should either be
   unconditional whenever retention runs or not built.

6. **How is any of this measured, given the frozen fixture contains none of the
   relevant data — and what happens to 014's published constants if the fixture
   changes?** — *flagged by: feasibility, gaps, requirements, stakeholders*
   **[cross-dimension]** *(highest doubling risk)*

   `build_retention_fixture` inserts every `tool_actions` row with
   `tool_name = 'Read'`, `category = 'tool_detail'`, and `full_input`,
   `lines_added`, `lines_removed`, `agent_id` all NULL. So the fixture contains
   **zero** rows `get_batch_session_code_stats` can read (it filters
   `category = 'code_change' AND full_input IS NOT NULL`) and **zero** rows with
   an `agent_id`. Every measure this feature would carry is absent from the only
   corpus the spec permits.

   Extending the fixture to carry `full_input` — the widest column in the table —
   changes page count, row width, and index churn, which **invalidates every
   published constant in the shipped delete engine**: the 1,289,674,752 B file
   size, `RETENTION_WAL_BYTES_PER_ROW = 788.7`, the 25,000-row chunk size, and
   both preflight terms. That is a spike re-run plus a rewrite of the engine's
   budget constants plus their tests — entirely unscoped here.

   Two further measurement traps: the rollup scan is a *different IO shape* from
   the measured Counting scan (Counting is index-only and never touches a base
   row page; rollup needs payload columns, so it is rowid-driven random access
   with no covering index — production `tool_actions` averages ~4.3 KB/row
   against a fixture whose rows are a fraction of that, so the fixture will
   under-report by roughly an order of magnitude). And the code-change measure
   **cannot be a SQL `SUM()`** at all: rows predating migration 33 have NULL
   stored counts and the reader re-parses `full_input` JSON in Rust, so the
   rollup must stream and parse every doomed `code_change` row while holding the
   quiesce lease.

7. **Do aggregates replace or add to the raw rows that survive below the cutoff
   — and what deletes an aggregate?** — *flagged by: ambiguity, stakeholders,
   scope, gaps* **[cross-dimension]**

   Three classes survive below the cutoff: live rows (`source_key IS NULL`),
   non-conforming timestamps (the `length(timestamp) = 24 AND timestamp LIKE '%Z'`
   guard), and — after a `partial` run — every row in whichever target the drain
   never reached. *Add* under-reports; *replace* erases live rows, the class 014
   calls genuinely unrecoverable. `get_batch_session_code_stats` has **no time
   filter at all**, so for that reader there is no natural seam to splice at.

   Separately, nothing in the spec deletes aggregates.
   `delete_session_data`/`delete_project_data`/`delete_host_data` all route
   through `suppress_transcript_analytics_sources_in_transaction`, which deletes
   by `(provider, source_key)` across both target tables — a user deleting a
   session for privacy would leave its numbers behind in the rollup table. This
   also breaks 014's load-bearing counter-pressure argument that "every surviving
   owned row is transcript-backed": aggregate rows would be the first analytics
   rows in this database that are neither transcript-backed nor reachable by
   reconciliation. That is a new data class the spec does not name.

### Non-Blocking Observations

- **The consent preview is untouched by the spec.** With rollup on, the run
  *writes* rows and *grows* the file before shrinking it — a different bargain
  from what `RetentionPreview` currently describes. If the preview gains a rollup
  estimation pass, `RETENTION_STALE_PREVIEW_TOLERANCE_MS = 2_616` must be
  re-derived.
- **No progress phase.** The `&'static str` phase vocabulary
  (`Counting rows`/`Checking disk space`/`Removing old rows`/`Compacting database`)
  is closed and mirrored in frontend mocks. Rollup is a new phase with no natural
  pct source, inside a lease already flagged as uncancellable.
- **The audit-record extension is nearly free — take the free version.** Adding
  `aggregate_rows_written` with `#[serde(default)]` and **no** bump to
  `RETENTION_AUDIT_SCHEMA_VERSION` keeps older builds reading the record; bumping
  makes them discard the whole record.
- **`RetentionDeleteControls` already is the extension point** (`after_chunk`,
  `free_space`, both progress sinks). Goal 6 should be written against it. But
  the shape matters: a callback-after-scan serves one consumer, while two
  consumers reading the same doomed range each pay a full payload scan unless the
  hook is a *row stream* both subscribe to. Retrofitting a visitor after a
  callback ships is a rewrite of both consumers.
- **`quill-uhg` may dominate this feature outright.** A JSONL sidecar of the
  doomed rows loses nothing (vs. "an aggregate is lossy by design"), needs no
  schema, no reader change, no `aggregated` span state, and — because nothing
  reads it — no watermark re-ordering. If the goal is "the user does not lose
  their history", export answers it more completely for less. Also note the
  contract mismatch: uhg's bead says it must cover "the rows the preview counts,
  including the non-conforming-timestamp classes", but 014 counts those *because
  they are retained, not deleted*.
- **The `straddles` state is silently dropped.** `RetentionSpan` is
  `retained | straddles | pruned`; Goal 5 and US3 enumerate
  `retained | aggregated | pruned`. `straddles` combined with `aggregated` is the
  common case for any long session, and `straddles` is exactly what
  `BreakdownPanel` consumes via `markPrunedRange`. Also, `retentionSpanFor` is a
  pure timestamp/cutoff function with no knowledge of aggregate existence — it
  cannot return `aggregated` without a new input.
- **Coverage cannot be inferred from row existence.** Rollup is opt-in and
  non-backfilling, so the band below the watermark is a patchwork of pre-ship
  runs, rollup-off runs, and rollup-on runs. Under a "does a row exist for this
  day" heuristic, a genuinely zero-activity day is indistinguishable from an
  un-aggregated one. A coverage ledger is a second schema decision.
- **DESIGN.md has no affordance for "coarse but real".** 014 solved the honesty
  problem by not rendering a number at all (`PRUNED_PLACEHOLDER`: "a zero is a
  measurement, a dash is an admission"). Rollup introduces a figure that renders
  exact and is not, in tabular Geist Mono, under a "Mono-for-Truth" rule and a
  PRODUCT.md principle that "trust is the product". The severity ramp is
  prohibited. Specify the affordance rather than deferring it.
- **Row-level accessibility copy is unextended.** `BreakdownPanel` appends
  "(incomplete, pruned by retention)" to row `aria-label`s; `ResultCard` and
  `DetailPanel` carry explanatory `title=` text. A coarse aggregate replacing an
  em dash is visually indistinguishable from a raw number and needs a non-visual
  marker.
- **A non-IPC consumer exists.** `learning.rs#select_sessions_for_insights` calls
  `Storage::get_session_breakdown` directly, bypassing the command layer. An
  aggregate-splicing branch there silently changes learning's session selection.
  Relatedly, `lat.md` records that a learning pipeline sourcing from
  `tool_actions` would make retention a stakeholder and force the whole analysis
  to be redone — an aggregate table is exactly the shape such a pipeline prefers.
- **Quill already has a rollup vocabulary.** `aggregate_and_cleanup` and
  `aggregate_and_cleanup_tokens` roll `usage_snapshots`/`token_snapshots` into
  `usage_hourly`/`token_hourly` on a hard-coded 30-day cutoff, automatically.
  Shipping a second, incompatible rollup grammar is a coherence cost worth
  naming, given 014's "not a general data-lifecycle framework" Non-Goal.
- **OQ9 has a forced answer.** Non-conforming rows are excluded from the scan and
  survive below the cutoff, so rolling them up double-counts them against their
  own surviving raw rows. Exclude them, and accept that aggregates disagree with
  raw counts in that band — the existing "may be incomplete" copy already covers
  it.
- **Missing Non-Goals:** not a new >30-day range preset (without one, aggregates
  are unreachable by construction); not aggregate/raw reconciliation; not
  backfill of the `[watermark, next cutoff)` band for someone enabling rollup
  after a prune but before the next run; not Tantivy/MCP search integration.
- **MCP is correctly not a stakeholder.** Verified: `search_history` and the
  server's search/context/facets routes all resolve through Tantivy, with no SQL
  read of either target table. Rollup does change the documented asymmetry
  though — today a search hit survives with an empty drilldown; after rollup the
  drilldown becomes coarse, so the `session-search` banner footnote needs
  revising.
- **Population sizing argues for the cheap mitigation.** A beneficiary must
  enable retention, enable rollup, and do both before their first prune — and
  014 shipped with `never` as the default on every database. The urgency argument
  ("the first users to enable it are about to create the first permanent holes")
  favors shipping the cheapest mitigation immediately, not the largest one.
- **Predictable day-after asks**, in order: backfill my already-pruned window
  (no product answer exists, only a disclaimer — put a stock line in the UI
  copy); let me drill into an aggregate; why does March disagree with the raw
  rows; **now do `model_usage_observations`** (`quill-i8b` is ~1.07 GB, i.e. more
  absolute value than this entire feature, and is arguably the better next bead);
  retention for the aggregate table.
- **Doc obligations.** Shipping this invalidates load-bearing assertions in
  `lat.md/backend.md#Retention pruning` (the "three settings rows, no table, no
  migration" property), `lat.md/frontend.md#Retention Degradation`, and
  `lat.md/frontend.md#All-Range Retention Invariant`. This repo gates on
  `lat check`, so list them as deliverables.
