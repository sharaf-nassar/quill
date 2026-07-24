# Analysis: analytics-query-perf

## Coverage Table

| User story / requirement | Covered by (plan section) | Status |
|--------------------------|---------------------------|--------|
| S1 cached instant switch + SWR + TTL | Frontend `useCachedInvoke` (Architecture §2), hook ports (Sequencing) | full |
| S1 per-table event invalidation | Event→hook subscription map (Architecture §2) | full (added in alignment round) |
| S2 race-free rapid switching | 200 ms debounce + generation guard in shared hook | full |
| S3 Models backend cache, sliding-window-safe key | In-process cache keyed (command, range, provider, time-bucket) + TTL + per-table version probes (Architecture §1) | full |
| S3 probes cover out-of-process writers + backfill | Absolute per-table probes (COUNT+MAX), reliability spike with <5% cost threshold and MAX-only fallback | full |
| S3 cold-call cost reduction | Benchmark rejects the direct per-statement CTE replacement: it takes 16,838 ms versus 5,803 ms for the indexed shared temp set and preserves identical result sets | full (decision recorded) |
| S4 no duplicate command+args per switch | Dedupe items: `get_llm_runtime_stats` collapse, SnapshotGate lift-to-parent | full |
| S4 `get_token_hostnames` not range-refetched | Hoist item | full |
| S4 `useCodeInsights` series reuse | Concrete target: 0 new overlapping history queries when cache-warm | full (hedge removed) |
| S5 migration v34 drop, idempotent, one-way door documented | Migration item; release-note + migration-comment text folded into acceptance | full |
| S5 compact/VACUUM operation | `compact_database` on dedicated connection: disk preflight, ingest quiesce, progress events, skip-and-report; button-only (idle-trigger scoped out of MVP) | full |
| S5 ingest safety during VACUUM | Standalone "Ingest quiesce guard + retriable ingest boundary" item; compact command depends on it | full (added in alignment round) |
| S6 timeframe-proportional breakdown queries | Deferred per Clarifications Q6 | deferred |
| S7 N+1 / multi-pass collapse | Cache-only per Q6 ("served from cache on repeat within TTL"); rewrites deferred | full (as re-scoped) |
| Q4 timing logs, no CI harness | Timing-log workstream; measurement pass files follow-up bead if guidance targets missed | full |
| Q7 SWR everywhere incl. Now tab | Recorded: single TTL constant, no special-case exemption | full |
| Failure paths (probe error, query error, VACUUM abort) | Fail-open probe miss + warn; query errors propagate (no silent stale); skip-and-report | full (added in alignment round) |
| Docs: lat.md sync | Final "Document in lat.md + pass lat check" item depending on all implementation items | full (added in alignment round) |

## Remaining Risks

- **Cache-probe cost on large tables.** `SELECT COUNT(*)` is a full-table
  scan in SQLite. Mitigated by the spike's <5% cost threshold and the
  documented MAX-only fallback (deletes then bounded by TTL). Residual:
  fallback misses hard deletes inside the TTL window — accepted.
- **Quiesce boundary completeness.** The maintenance guard must be honored
  by every writer (HTTP ingest, backfill worker). A missed writer during
  VACUUM risks a failed write past the 5 s busy timeout. Mitigated by the
  standalone quiesce bead with a paused/retried-never-dropped test.
- **Cold Models-tab cost still misses guidance.** The direct CTE replacement
  was materially slower and is rejected; a single-statement endpoint redesign
  remains a separately scoped option if the ~1.5 s cold guidance is required.
- **One-way schema door.** v34 bump means older builds refuse the DB; no
  downgrade. Accepted per Clarifications Q5 (single-user local app);
  documentation folded into the migration bead's acceptance.
- **No automated frontend tests this round.** Hook-port "done" =
  typecheck + lint + manual dev-mock verification; vitest is an
  approval-gated follow-up. Race/SWR regressions rely on manual checks
  until then.

## Unresolved Questions

- Exact TTL constant (30 vs 60 s) — left to implementation within the
  spec's stated band.
- Compact-database button placement within the existing settings/systems
  surface — concrete close condition recorded; exact placement decided at
  implementation, no redesign permitted.

## Constitution Check

No constitution.md — skipped.

## Recommendation

**GO** — Every in-scope requirement and all seven clarification answers
trace to plan items with closeable acceptance criteria; both independent
review passes' must-fix findings were applied in the alignment round
(per-table event map, quiesce split, fail-open probe path, lat.md item,
concrete targets replacing hedges). The two partial/deferred rows are
deliberate, human-approved scope decisions (Clarifications Q6) with
recorded re-entry conditions, not gaps. Remaining risks all carry
mitigations or explicit accepted-residual notes.
