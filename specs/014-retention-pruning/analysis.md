# Analysis: retention-pruning

## Coverage Table

| User story / requirement | Covered by (plan section) | Status |
| --- | --- | --- |
| **G1** Non-destructive reclaim first (Phase 1 index drop, ~473 MB) | Architecture Approach § Phase 1 *Index drop*; Sequencing "Prove index-drop query plans" → "Drop the redundant provider/source index and measure" | partial — reclaim is gated on the `EXPLAIN QUERY PLAN` proof; a fail closes the drop bead as won't-do and returns 0 MB |
| **G2** Bounded steady-state size for the two target tables | Architecture § *Goal 2's bound is conditional*; Risks § "Goal 2's bound depends on the user re-running"; Sequencing "Audit record surfacing" | partial — plan honestly downgrades the bound to "holds only under periodic manual re-runs"; mitigation is informational (last-run age vs window), no scheduler by Non-Goal |
| **G3** User-visible reclamation (before/after, what was removed) | API § `RetentionMaintenanceResult` (`bytes_before`/`bytes_after`, per-table counts); Data Model § `retention.last_run`; Sequencing "Audit record surfacing" | full |
| **G4** Reference reduction target (≈1.9 GB off 7.54 GB at 90d) | Sequencing "Drop the redundant provider/source index and measure" (*observational, not pass/fail*); Testing § frozen synthetic fixture | partial — by design: acceptance is the fixture's exact-count assertion; the GB figures are recorded as observations, nothing asserts ≈1.9 GB |
| **G5** No silent breakage of readers | Architecture § *Consumer degradation is the cheap treatment*; Affected Components § Consumer degradation surfaces; Sequencing "Consumer degradation treatment" | full — all seven readers from the spec's Constraints table are dispositioned (three provably unaffected via the 30-day floor, three banner/truncate, `subagent_count` accepted) |
| **G6** Pruning is durable | Architecture § *Durability is an insert-time watermark*; Data Model § watermark monotonicity and advance timing; Testing (S5a/S5b regressions, kill-after-chunk-*N*) | full |
| **G7** Bounded write-lock impact | Architecture § dedicated maintenance connection, chunked delete, `wal_checkpoint(TRUNCATE)`; Sequencing "Retention timing spike" | partial — mechanism fully specified; every numeric budget (chunk size, per-chunk hold, total wall time) is spike output |
| **G8** Composes with compaction | Architecture § *One composite operation*; API § `compaction_status` reported separately from `status`; Sequencing UI close criteria | full |
| **S1** Bounded database size under a retention window | Data Model § preset list + cutoff derivation; Architecture § *Timestamp non-conformance: retain and report*; Testing § frozen fixture, chunk correctness/idempotency, edge states | full — S1's open either/or is resolved to retain-and-report, with `skipped_nonconforming` surfaced in preview, result, and audit |
| **S2** Disk space actually comes back | Architecture § composite operation; API § `compaction_status` / `bytes_after`; Testing § preflight distinctness; UI close criterion carrying the "deletion alone frees no bytes" copy | full |
| **S3** Pruning does not stall or break the running app | Architecture § one lease via `try_begin_ingest_quiesce()`, dedicated connection, chunked deletes, delete-phase preflight, `progress_handler` heartbeat; Testing § quiesce interaction, interrupted run, `partial` | partial — all five acceptance criteria have owned mechanisms, but the "numeric budgets fixed by a timing spike" criterion is satisfied only by scheduling the spike; widget staleness is a close criterion, not yet observed |
| **S4** Consumers keep telling the truth | Architecture § cheap treatment, *`all` relabel dropped*, cache invalidation; Sequencing "Analytics cache invalidation on prune", "Consumer degradation treatment" | full — with one documented deviation: the `all` → "all retained" relabel does not ship (no `all` member in `RangeType`); the requirement survives as a recorded forward-looking invariant |
| **S5** Pruned data stays pruned | Architecture § insert-time watermark; Affected Components § `replace_transcript_analytics_snapshot`; Testing § normal-path resurrection + forced-reparse regressions | full |
| **S6** The user understands and controls the trade | Architecture § *Consent must be consent to a capability loss*; API § `preview_retention` / `confirmed_cutoff` binding; Data Model § `retention.last_run`; Sequencing settings-UI item | full — strengthened beyond spec: the backend makes a destructive run unreachable without a preview |
| **Q1** Ranked levers, one epic, two phases | Architecture Approach (Phase 1 / Phase 2 split); Sequencing (Phase 1 items independent, land first) | full |
| **Q2** Insert-time watermark in `settings`; siblings keep full history | Data Model § three settings keys; Affected Components § `replace_transcript_analytics_snapshot`; Testing § watermark filters snapshot inserts | full |
| **Q3** Opt-in, live rows excluded, no export | Data Model § `retention.window_days` absent by default; Affected Components § `server.rs` — no change; Testing § live rows survive (insert + delete halves) | full |
| **Q4** Age-based, presets, manual, composite, row-scoped, edge states | Data Model § preset list and cutoff derivation; API § four commands; Testing § edge states | full — plan adds a 30-day preset floor not in the spec text, argued from `range_to_duration` and enforced by `set_retention_policy` validation |
| **Q5** Cheap degradation, no rollup aggregates | Architecture § cheap treatment; Sequencing "Consumer degradation treatment", "File the deferred follow-up beads" | full |
| **Q6** Whole-run quiesce, chunked deletes, delete-phase preflight, spike | Architecture § one composite operation, doomed-rowid TEMP tables, `:max` chunk bound, WAL checkpointing; Sequencing "Retention timing spike" | full — chunking uses a materialized `:max` boundary rather than the spec's literal `rowid IN (SELECT … LIMIT ?)`; still no `DELETE … LIMIT`, and the change is argued (two unordered `LIMIT` scans are not guaranteed to agree) |
| **Q7** Exact preview counts, whole-file bytes, persisted audit record | API § `RetentionPreview`; Data Model § `retention.last_run`; Testing § preview accuracy, audit round-trip | full |
| **OQ11** Cache invalidation channel | Architecture § *Cache invalidation (OQ11, resolved)*; Sequencing "Analytics cache invalidation on prune" | full — resolved to unconditional five-cache clear plus `transcript-analytics-updated` |
| **OQ12** Should ingest write less (`tool_detail` payload policy)? | Sequencing "Decide the tool_detail payload write policy" → "Apply the tool_detail payload write policy" (conditional) | partial — owned by a decision bead; the grep is not closed and the outcome may be won't-do |
| **OQ13** Delete-phase timing spike numbers | Sequencing "Retention timing spike"; Architecture § *Budgets come from a spike, not from this document* | partial — the spike hard-blocks the delete engine, but every constant it produces is currently unknown |
| **OQ14** Is the measured corpus representative? | Risks § "One-machine corpus"; Sequencing "File the deferred follow-up beads" | partial — stays open and non-blocking, filed as a follow-up bead; acceptance is pinned to the frozen fixture instead |
| Not-a-question: EQP proof as a pass/fail task | Sequencing "Prove index-drop query plans" (hard gate on the drop item) | full |
| Not-a-question: no `constitution.md` | — | full (see Constitution Check) |

**Headline: 25 rows — 17 full, 8 partial, 0 none.** Every partial is either a
deliberate deferral recorded as a Non-Goal/follow-up bead, or a value the plan
routes to an owned prerequisite item (spike, decision bead, EQP gate).

## Remaining Risks

- **The EQP proof can fail, and Phase 1's value is binary.** All ~473 MB of
  the non-destructive target sits behind one gate covering three
  `(provider, source_key)` delete sites (storage.rs:2225, :3339, :3457). The
  plan's mitigation — "report the finding, close the drop as won't-do" — is
  an outcome, not a mitigation. Phase 1 would then reduce to a
  possibly-won't-do payload decision. Phase 2 is unaffected, so this is a
  scope risk rather than a schedule risk.
- **Spike output can invalidate the composite single-lease shape.** Two
  named triggers: per-chunk wall time unacceptable at every viable chunk
  size, and the Counting scan dominating the run (the design pays for a full
  `tool_actions` scan twice — preview and run — because `tool_actions` has no
  timestamp-leading index). The plan names the fallback (preview takes the
  lease and hands the run its materialized doomed set) but does not design
  it, so a fallback trip means re-planning the two most expensive items.
  Mitigated to the extent that the spike is sequenced ahead of the delete
  engine.
- **Lease duration is unbounded and uncancellable.** Scan + delete + a VACUUM
  measured at 82.5 s on a 7.45 GB fixture, under one lease, with no abort
  affordance anywhere in the command surface. Reads keep serving on the
  primary connection (real mitigation) and hooks retry `503`, but a user who
  starts a prune on a large database cannot stop it, and the
  widget-staleness claim is a UI close criterion rather than a measured
  fact. Partially mitigated.
- **`subagent_count` mixed horizons.** Accepted and documented, not fixed;
  the fix (rollup aggregates) is a deferred follow-up. A pruned session's
  count can contradict its own empty drilldown. Unmitigated by design.
- **Downgrade re-ingests pre-cutoff rows.** An older build has no watermark
  filter, so any source it reparses restores that source's full history. The
  plan accepts and documents this as the price of the no-migration posture.
  Correctly traded, but it is a real hole in "pruned data stays pruned" for
  anyone who downgrades.
- **Goal 2's bound is informational only.** No scheduler by Non-Goal, so the
  footprint bound holds only on days the user re-runs; the mitigation is a
  rendered "last pruned N days ago" string. Adequate for MVP, but the goal
  as written in the spec is stronger than what ships.
- **One-machine corpus and the timestamp-uniformity claim.** The 24-char
  uniformity assertion for these two tables rests on a single developer
  database (013's uniformity spike did not cover them). Well mitigated: the
  retain-and-report conformance guard is symmetric on the insert and delete
  sides, with a test on each half, so a non-conforming corpus degrades to
  "prunes less" rather than "mis-compares".

## Unresolved Questions

- **Every numeric budget** — chunk size, per-chunk wall target, WAL- and
  TEMP-bytes-per-row preflight constants, free-space re-check interval `N`,
  stale-preview tolerance, Counting-phase budget, total wall-time budget.
  All are spike output; none may be hard-coded first.
- **`PRAGMA temp_store` for the maintenance connection** (`MEMORY` trades
  disk for RSS; `FILE` puts the TEMP term on a possibly different
  filesystem). Explicitly the spike's choice to make.
- **Whether the two-scan / two-lease split survives.** Conditional design
  decision triggered by the spike's Counting-phase numbers.
- **The `tool_detail` payload write-policy outcome.** The grep is not closed;
  the result may be omit-columns, stop-writing-rows, or keep-as-is (which
  closes the conditional apply item as won't-do).
- **The EQP proof result itself.** Pass/fail unknown until run.
- **Second-corpus validation (OQ14).** Open and non-blocking; strengthens the
  defaults, does not gate the build.
- **`affected_surfaces`: payload field or static UI list keyed off the
  cutoff.** Left as an explicit either/or, owned by the settings-UI item.
- **Minor:** "Audit record surfacing" may fold into the settings-UI item;
  bead granularity is left to implementation.

## Constitution Check

No constitution.md — skipped.

## Recommendation

**GO** — The plan is internally consistent, grounded in verified line
references throughout, and every one of the seven binding Clarifications is
implemented rather than restated. All 25 tracked requirements are covered;
the eight partials are honest deferrals with named owners: budgets to a
sequenced spike that hard-blocks the delete engine, the `tool_detail` policy
to a decision bead with a conditional apply item and an explicit won't-do
branch, second-corpus validation and rollup aggregates and export to filed
follow-up beads, and the ~473 MB index drop to a pass/fail gate whose failure
is pre-declared as a finding. The plan also improves on the spec where the
code contradicts it — dropping the vacuous `all` relabel while preserving the
requirement as an invariant, adding a 30-day preset floor that makes "three
range readers are provably unaffected" enforceable at the command boundary,
and binding `run_retention_maintenance` to a previewed `confirmed_cutoff` so
consent is backend-enforced rather than UI-enforced. Two items to reconcile
during implementation rather than before approval: the 30-day floor narrows
Q4's "user-configurable presets" and the dropped `all` relabel narrows S4, so
spec.md should be annotated when those land. The residual risk that would
most change the shape of the work — the spike invalidating the single-lease
composite design — is correctly sequenced to surface before any of the
expensive implementation commits to it.
