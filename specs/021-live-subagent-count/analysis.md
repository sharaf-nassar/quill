# Analysis: live-subagent-count

## Coverage Table

| User story / requirement | Covered by (plan section) | Status |
|--------------------------|---------------------------|--------|
| Show root-linked observed-open subagents per Sessions row | Architecture Approach; Data Model; Sequencing: backend and frontend | full |
| Exclude main session and remove agents after observed stop | Data Model state transitions; backend fold tests | full |
| Preserve null / zero / positive truth states | Data Model; API / Interface Changes; backend acceptance | full |
| Never reconstruct positive state after restart or from audit history | Architecture Approach; Data Model; Risks | full |
| Accept the documented missed/blocked-stop best-effort boundary | Architecture Approach; Non-Goals trace; Risks | full |
| Count all root-linked descendants without direct-ancestry claims | Architecture Approach; Data Model; provider work items | full |
| Cover Codex lifecycle payloads | Affected Components: Codex; Sequencing: Codex observation | full |
| Cover Claude Code lifecycle payloads and managed hooks | Affected Components: Claude; Sequencing: Claude observation | full |
| Remove historical `has_subagents` / `subagent_count` projection and SQL | Affected Components: backend/frontend; backend acceptance | full |
| Render positive neutral `+N` beside project name | API / Interface Changes; Sequencing: frontend | full |
| Hide null/zero and remain independent of `row.live` | Testing Strategy; frontend acceptance | full |
| Accessible singular/plural observed-open meaning | API / Interface Changes; Testing Strategy | full |
| Preserve 320 px layout and exact numerics | Testing Strategy; frontend acceptance | full |
| Refresh from existing invalidation without polling | API / Interface Changes; Testing Strategy | full |
| Keep session-breakdown p95 at or below 300 ms | Testing Strategy; final verification acceptance | full |
| Preserve user-owned hook config and rollback | Claude/Codex affected components; Risks; compatibility text | full |
| Add only authorized lifecycle/provider/IPC/UI regression tests | Testing Strategy; Clarification Q4 trace | full |
| Synchronize lat.md and one-to-one test specs | Documentation and verification; final task acceptance | full |
| Exclude teams, Agent View, background commands, drilldown, totals, history, process probing, TTL, and LIVE cleanup | Architecture alternatives; scope/non-goal trace | full |

## Backlog Disposition

| Source P4 id | Plan work item(s) / non-goal | Disposition | Ready to resolve? |
|--------------|-------------------------------|-------------|-------------------|
| None | Five new P1/P2 work items; closed investigations `quill-kbb` and `quill-pwu` remain source context | no backlog input | yes |

## Target Epic

New epic: `Show observed subagent counts in Sessions`.

The epic will contain five implementation-ready tasks. Backend monitor, Claude
observation, and Codex observation are initially dispatchable; frontend depends
on backend; final architecture/verification depends on all implementation work.

## Remaining Risks

- **Missed or blocked stop:** accepted hook-observed boundary may leave a
  temporary stale positive. Mitigation: honest observed-open contract,
  process-local reset, parent-end clearing, and no verified-liveness claim.
- **Provider schema drift:** unknown source or missing identity must remain null.
  Mitigation: official allowlist evidence, payload/setup tests, and fail-closed
  root-local invalidation.
- **One-shot observer delivery:** network failure is not externally detectable.
  Mitigation: documented best-effort scope; no unsupported durable-delivery
  promise or bead.
- **Registry saturation:** fixed caps can make later roots unknown. Mitigation:
  preserve existing watermarks, never evict into false coverage, and recover on
  restart/new epoch only.
- **Hostname mismatch:** exact cross-host join can fail closed. Mitigation: reuse
  the token reporter's short-host normalization and test an FQDN case.
- **Manual corpus harness compatibility:** archived measurement harness may not
  compile against final code. Mitigation: treat incompatibility as a verification
  blocker and report it; do not add unauthorized benchmark test code.
- **Parallel integration:** three root tasks intentionally run together.
  Mitigation: file ownership is partitioned; backend owns shared Rust contract,
  provider tasks own separate scripts/installers, and frontend waits for backend.
- **Local delivery authority:** Molecule must commit/squash spec artifacts and
  final Beads audit log to main. Mitigation: require explicit authority in the
  human GO decision; never push.

## Unresolved Questions

None. On 2026-08-05, the human selected **GO** and explicitly authorized the
required local commits to main for `specs/021-live-subagent-count` and the
Beads audit log. Push remains out of scope.

## Constitution Check

1. **Local source-backed truth — pass with accepted tension.** Nullable
   current-boot evidence and observed-open naming avoid invented certainty;
   undetectable missed stops remain documented.
2. **Established stack and boundaries — pass.** Rust/Tauri owns state and IPC;
   strict TypeScript/React renders it.
3. **Responsive execution — pass.** Bounded memory overlay replaces historical
   SQL enrichment; no setup/UI blocking or polling is added.
4. **Recoverable mutation — pass.** Managed hooks remain additive,
   transactional, last-known-good preserving, and rollback needs no data change.
5. **Typed failure boundaries — pass.** Malformed/unsupported evidence fails
   closed per root and keeps contextual diagnostics.
6. **Zero-warning quality gates — pass by plan.** Full Rust/frontend/release
   gates and diff checks are final acceptance.
7. **Authorized behavior testing — pass.** User selected Option A and authorized
   the scoped lifecycle/provider/IPC/UI regression tests; no new performance
   test code is planned.
8. **Architecture traceability — pass by plan.** Final task owns lat.md updates,
   one-to-one test refs, and `lat check`.
9. **Glass Cockpit discipline — pass.** Plain neutral exact metadata, existing
   row grammar, accessible text, no chrome/color/motion.
10. **Measured performance — pass by plan.** Existing invalidation latency and
    frozen-corpus p95 have reproducible evidence requirements.
11. **Explicit external transmission — pass.** Hook traffic remains local to
    Quill's existing loopback endpoint; no off-device transmission is added.
12. **Gated delivery — pass.** Work is tracked in Beads, both human gates were
    observed, the GO authorized required local commits, and no push is planned.

## Recommendation

**GO** — The clarified spec and aligned plan fully cover the approved feature,
contain no unresolved backlog or target ambiguity, respect every constitution
principle, and reduce the durable implementation graph to five concrete P1/P2
tasks. The human approved Bead creation and the Molecule-required local spec and
Beads-audit commits to main; do not push.
