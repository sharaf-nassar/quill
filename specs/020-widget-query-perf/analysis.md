# Analysis: widget-query-perf

Report-only pre-bead analysis. Sources: six-dimension spec review, two-pass
alignment round (spec↔plan coverage + plan quality), live-DB measurements
(2026-08-02), and the bounded ingest-rate check (clarification Q6a).

## Coverage Table

| User story / requirement | Covered by (plan section) | Status |
|---|---|---|
| Goal 1 / S1 — Models ≤500ms cold at 30d/90d | Architecture (slice A), Data Model `model_usage_hourly`, Sequencing items 2-8, budget table | full |
| Goal 2 / S2 — runtime stats ≤200ms at 90d, time-invariant semantics | Architecture (slice B), items 9-10, 13; pinned-`now` parity harness | full |
| Goal 3 / S3 — no view query blocks another | API (slice C: all view-serving reads migrate; re-measurement sets priority), items 15-16, Family 3 | full (post-fix) |
| Goal 4 / S4 — refresh proportional; range×2 only over-range; ≥5s coalesce; skills range-scoped; cache survives view switch | Slice D, items 17-18, query-window logging deliverable | full |
| Goal 5 / S5 — full_input elimination, session_breakdown restructure, ANALYZE, ≤300ms budgets | Slice E, items 19-22 | full |
| Goal 6 — rollups tell the truth; retention interplay | Data Model fold-then-prune + `raw_pruned` for BOTH `model_usage_hourly` and `runtime_hourly`; item 6; Family 1 | full (post-fix) |
| Goal 7 — reproducible evidence | Item 1 (harness + frozen 13.5 GB corpus), 14 (re-measure), 22 (timing doc) | full |
| Non-Goals (7) | Scope-creep audit: none found | full |
| Clarifications Q1-Q7 | All traced; Q1 deviation noted: suppression handled by read-time join instead of row invalidation — strictly stronger (flips cost nothing, exactness preserved) | full |
| Spec Review CQ1-CQ7 | All addressed in plan; CQ4 runtime-side rebuild/retention added in alignment round | full |
| Constitution 12 principles | Checked in plan with explicit tensions (1, 3, 10) | full |

## Backlog Disposition

| Source P4 id | Plan work item(s) / non-goal | Disposition | Ready to resolve? |
|---|---|---|---|
| (none) | No P4 sources existed (no epic, no source_backlog; bd search empty) | n/a | yes |
| quill-xnb (P1 bug, not P4) | Referenced as sizing dependency only; fix NOT in scope; follow-up bead "re-run sizing/consistency validation after xnb fix" filed at create-beads | not-in-scope | yes |
| .bak communication | Follow-up bead at create-beads (owner assignment) | follow-up | yes |

## Target Epic

New epic, created at create-beads. No candidates existed; no ambiguity.

## Remaining Risks

- **Burst-sizing uncertainty**: true current ingest throughput is understated
  ~100× by quill-xnb (source-admission regression). Mitigated: rollup sized
  for the measured 866k rows/day burst envelope + revalidation bead after
  the xnb fix. Residual: if post-fix throughput exceeds ~1M rows/day
  sustained, the open-hour raw tail read grows past its ~36k-row bound.
- **Ingest write-path overhead**: same-transaction UPSERT fold adds work to
  every ingest commit; budgeted ≤10% added p95 (items 3, 9) with the
  fold-moves-post-commit escape hatch if the budget fails.
- **ANALYZE app-wide plan shifts**: code today relies on a stat-free
  planner. Mitigated: ANALYZE is last in slice E (item 21), gated on an
  EQP audit of hot queries, `analysis_limit=1000`, and a documented
  `DELETE FROM sqlite_stat1` rollback.
- **One-way schema door**: migration 36→37 is additive DDL only; backfill
  out-of-band and resumable; older builds hard-refuse (SCHEMA_TOO_NEW) —
  accepted UX, documented.
- **Frozen-corpus logistics**: needs ~14 GB disk for the benchmark
  snapshot; recorded in the protocol (item 1). OS page cache remains
  uncontrolled — recorded as a caveat, cold defined as first in-process
  call with app caches bypassed.

## Unresolved Questions

None blocking. Two deferred by design: (a) post-rollup cleanup of the
3.6 GB `model_usage_observations` index set — explicit follow-up outside
this feature; (b) whether C/D/E shrink after the item-14 re-measurement —
that is the point of the gate.

## Constitution Check

| # | Principle | Verdict |
|---|---|---|
| 1 | Local source-backed truth | tension, resolved: rollup becomes authoritative ONLY for retention-pruned windows, via exact fold-then-prune + `raw_pruned`; everywhere else raw remains truth and closed buckets are integer-exact |
| 2 | Established stack and boundaries | pass |
| 3 | Responsive execution | tension, resolved: backfill chunked, quiesce-yielding, disk-prechecked per chunk, resumable; ingest fold budgeted ≤10% p95 |
| 4 | Recoverable mutation | pass (transactional folds, bookmarks, rebuild commands) |
| 5 | Typed failure boundaries | pass (typed backfill terminal events incl. ENOSPC) |
| 6 | Zero-warning quality gates | pass (item 23 final gate) |
| 7 | Authorized behavior testing | pass — three families authorized at clarify gate, 1:1 lat.md links |
| 8 | Architecture traceability | pass (per-family lat.md items + final `lat check`) |
| 9 | Glass Cockpit discipline | pass — UI deltas limited to progress note + Performance-tab row (settings is a DESIGN.md §6 exception surface) |
| 10 | Measured performance | tension, resolved: budgets machine-relative on the frozen corpus; OS-page-cache caveat recorded |
| 11 | Explicit external transmission | pass (nothing off-device) |
| 12 | Gated delivery | pass (beads DAG, gated slices) |

## Recommendation

**GO.** Every spec goal, story, constraint, and clarification answer traces
to a concrete plan item with verifiable acceptance criteria; both alignment
reviewers' must-fixes are applied (independent A/B slices, runtime-side
retention semantics, cache-probe edge, ingest budget on the runtime fold);
no scope creep against the seven Non-Goals; no P4 anywhere; the target epic
is unambiguous (new). The one external uncertainty — true ingest throughput
behind quill-xnb — is contained by burst-envelope sizing plus an owned
revalidation bead, and does not change the architecture.
