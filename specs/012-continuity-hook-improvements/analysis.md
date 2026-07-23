# Analysis: continuity-hook-improvements

## Coverage Table

| User story / requirement | Covered by (plan section) | Status |
|--------------------------|---------------------------|--------|
| S1: trivial prompts never become `last_prompt` (heuristic: <12 chars OR single token) | API/Interface item 1; Sequencing item 2 | full |
| S1: fallback to newest non-trivial prompt; omit when none; capture unchanged | API/Interface fallback chain 3a–3c | full |
| S2: coherence-first anchor (newest session with non-trivial prompt AND ≥1 hint) | API/Interface item 2; Sequencing item 3 | full |
| S2: deterministic fallback order; 2026-05-18 mixed-hints shape never interleaves | API/Interface item 3; Testing fixture | full |
| S3: snapshot-first within anchor (covers `last_prompt` via `prompt_summaries` and hints); event-path degrade | API/Interface item 2 | full |
| S3: 256KB tail + 5s budget preserved | Architecture "Bounds preserved" | full |
| S4: retire 3 MCP tools; scrub README.md:67, server.py:51-52, CONTEXT_TOOLS both copies | Affected Components; Sequencing items 2, 5 | full |
| S4: keep taxonomy + tests, stats fallback, SQLite tables inert | Affected Components "Untouched" | full |
| Goal: telemetry replaces coverage floor (`source`, `trivialSkipped`, `coherent`) | Data Model; Sequencing item 2 | full |
| Goal: prune concurrency-safe (lockfile + atomic rename) | API/Interface item 4; Sequencing item 4 | full |
| Goal: no regression of per-project scoping + empty-gate | Testing Strategy regression cases (incl. worktree quirk) | full |
| Q7: fixture harness (bespoke runner mirroring context-router.test.cjs) + pure-helper exports | Testing Strategy; Sequencing item 1 | full |
| lat.md sync (features.md:203 rewrite, backend.md, infrastructure.md correct heading) + `lat check` green | Sequencing item 7 | full |
| Byte-identical Codex copy + cmp test; hash-gated redeploy verification | Sequencing item 6 (depends on 2,3,4) | full |

All 9 spec Non-Goals respected — no scope creep found by either review pass.

## Remaining Risks

- Silent-failure design can hide selection regressions in production;
  mitigated by the harness, exported pure helpers, and QUILL_DEBUG assertions.
- Injection coverage will drop (stricter filters route more sessions into the
  empty-gate); explicitly accepted per Clarifications Q5 and observable via
  the new telemetry fields.
- Lockfile staleness edge (crashed holder); mitigated by ~30s stale-steal +
  proceed-anyway fallback so capture never hard-fails.
- Dual-copy drift between Claude and Codex script copies; mitigated by the
  cmp byte-identical test and item-6 dependency on all cjs-touching items.
- Rollback = revert script bytes; hash-gated redeploy restores prior content
  (verified mechanism in manager.rs:461).

## Unresolved Questions

None. All seven clarify-gate questions were answered (delegated to the
assistant 2026-07-23, grounded by code investigation); both alignment passes
found zero must-fix items and their eight should-fix suggestions are applied
to plan.md.

## Constitution Check

No constitution.md — skipped (human was offered a stop at the clarify gate
and delegated proceeding without one).

## Recommendation

**GO** — the plan traces every spec requirement, story criterion, and
clarification answer with no gaps or contradictions; both independent review
passes found the plan sound with only should-fix polish, all of which is
already applied; risks are enumerated with concrete mitigations and a real
rollback path. Bead creation can proceed directly from ## Sequencing, which
is already expressed as an explicit dependency DAG sized one bead per item.
