# Spec: observed-agent-cap-saturation

## Problem Statement

Quill bounds process-local observed-subagent state with two limits. The
`quill-9sy` marker at `server.rs:211` guards
`MAX_OBSERVED_AGENTS_PER_ROOT = 256`, not the distinct-root cap. A root epoch
tracks distinct agent IDs, including stopped agents; its 257th ID invalidates
that root until a newer qualifying root epoch.

`MAX_OBSERVED_ROOTS = 1024` separately bounds root keys. Ended roots remain
allocated until restart, and a new root is rejected when the registry is full.
The source P4 does not itself specify 1,024, so its cap identity resolves to the
256-agent limit named by its source marker.

A production read-only snapshot at `2026-08-05T04:16:41.881365Z` found schema
37 and 306,599 hook rows. Fold-eligible evidence contained 27 Claude rows over
3 roots (maximum 9 distinct IDs, peak 2) and 48 Codex rows over 12 roots
(maximum 7 distinct IDs, peak 3). Eight eligible roots existed since the latest
logged server boot. No real saturation is demonstrated.

Historical rows cannot prove saturation: source is not persisted, hostname is
missing for 4,439 lifecycle rows, no boot ID exists, five audit writes failed,
and audit persistence follows runtime folding. Hostname-agnostic Codex history
conflates epochs and boots; its apparent 389 IDs falls to at most 67 between
`SessionStart` boundaries and remains non-authoritative. No eviction, telemetry,
cap change, or other runtime work is justified.

## Goals

- Resolve `quill-9sy` as referring to the 256-agent per-root limit guarded by
  its `server.rs:211` marker, distinct from the 1,024-root registry limit.
- Preserve bounded memory, ordering watermarks, and null-on-unknown behavior.
- Treat existing synthetic saturation tests as correctness evidence only, not
  proof that real workloads reach either limit.
- Make no runtime change when trustworthy process-scoped saturation evidence is
  absent.
- Record the human-approved retirement of `quill-9sy` as an evidence-backed
  non-goal.
- Create only the minimum durable decision record needed to preserve the source
  provenance, evidence review, and no-code outcome.

## Non-Goals

- Implementing eviction, changing either cap, or altering lifecycle folding in
  this feature without the required real-workload evidence.
- Adding permanent counters, logs, analytics, telemetry, database columns,
  migrations, IPC, UI, remote reporting, or background collection to search for
  a hypothetical saturation event.
- Reconstructing current observed-subagent state from `hook_invocations` or
  treating retained audit history as a Quill process boundary.
- Treating a synthetic test that inserts 1,024 roots or 257 agents as production
  saturation evidence.
- Changing root epoch, provider disable, activity tracking, restart, ordering,
  hostname, or nullable Sessions semantics from Feature 021.
- Probing or modifying the user's live Quill window or mutating any local data.
- Adding measurement work, automated tests, or implementation tasks. This
  pipeline authorizes only the durable decision record.

## Backlog Inputs

### Source P4: quill-9sy

`quill-9sy` marks the `MAX_OBSERVED_AGENTS_PER_ROOT = 256` guard at
`server.rs:211` and asks for measured eviction only if real workloads saturate
that limit without a newer qualifying root epoch. It does not specify 1,024.

The source is unparented and has no `discovered-from` edge. Its provenance text
mentions closed epic `quill-k0x`, but text is not a hierarchy or dependency
edge. This run must preserve `quill-9sy` as provenance while creating a new
epic; it must not infer parentage from `quill-k0x`.

The human has approved retiring `quill-9sy` now as an evidence-backed non-goal
based on the recorded cap identity, production evidence review, and no-code
outcome. This pipeline may create only the minimum durable decision record and
zero implementation tasks.

## Target Epic

This Molecule run may create the minimum durable decision record titled
**Validate observed-agent cap saturation**. It must not parent that record
under `quill-k0x`.

Its only outcome is the human-approved retirement of source P4 `quill-9sy` as
an evidence-backed non-goal. It creates zero measurement or implementation
tasks. Any future measurement or runtime proposal requires separate
authorization and specification outside this pipeline.

## User Stories

### Identify the actual saturation boundary

As a maintainer, I want the backlog claim mapped to the shipped state model so
that work targets the real limit instead of conflating roots and agents.

Acceptance criteria:

- The decision record identifies `MAX_OBSERVED_ROOTS = 1024` as the number of
  provider/hostname/session root entries retained in one Quill process.
- The decision record identifies `MAX_OBSERVED_AGENTS_PER_ROOT = 256` as the
  number of distinct agent lifecycles retained for one root.
- It records that the `quill-9sy` marker at `server.rs:211` guards the 256-agent
  limit, resolving the source's intended cap.
- It records that a new root is rejected at the 1,024-root limit until process
  restart; a newer root epoch does not create an entry while the registry is
  full.
- It records that 256-agent overflow invalidates only that root and a strictly
  newer qualifying `SessionStart` may restore known coverage.
- Stopped agent IDs continue to consume that root epoch's 256 distinct-ID
  allowance.
- The 257th distinct ID invalidates that root until a strictly newer qualifying
  `SessionStart` restores known coverage.

### Use existing evidence to reject eviction

As a maintainer protecting Quill's source-backed truth, I want eviction gated
by real process-scoped saturation evidence so complexity cannot be justified by
a synthetic boundary test.

Acceptance criteria:

- The production snapshot records schema 37 and 306,599 hook rows at
  `2026-08-05T04:16:41.881365Z`.
- Fold-eligible evidence records Claude at 27 rows over 3 roots, maximum 9
  distinct IDs and peak 2, and Codex at 48 rows over 12 roots, maximum 7
  distinct IDs and peak 3.
- It records eight eligible roots since the latest logged server boot and
  concludes that neither shipped cap was saturated.
- Historical distinct-root totals without a process boundary, synthetic unit
  tests, estimates, and extrapolations fail the gate.
- Historical Codex evidence remains non-authoritative because audit rows omit
  source and boot identity, 4,439 lifecycle rows omit hostname, five audit
  writes failed, and persistence occurs after runtime folding.
- The apparent hostname-agnostic 389 Codex IDs, or at most 67 between any
  `SessionStart` boundaries, does not establish one root epoch or process.
- If existing local evidence cannot meet the gate, the result says
  **not demonstrated**; it does not add collection code to manufacture proof.
- The decision record retires `quill-9sy` without measurement work, eviction,
  cap changes, telemetry, runtime code, or automated tests.

### Preserve the safe no-code outcome

As a Quill user, I want trustworthy nullable counts and bounded memory retained
when no real saturation is demonstrated so an unneeded eviction policy cannot
discard ordering evidence.

Acceptance criteria:

- When the evidence gate fails, `MAX_OBSERVED_ROOTS`,
  `MAX_OBSERVED_AGENTS_PER_ROOT`, `ObservedSubagentState`, and existing tests
  remain unchanged.
- Saturation continues to fail closed: existing known roots retain their state,
  an untracked root stays null, and per-root overflow stays null until a newer
  qualifying epoch.
- No telemetry, database, IPC, UI, configuration, or `lat.md` behavior change is
  created for the no-code result.
- The decision record records human approval to retire `quill-9sy` as an
  evidence-backed non-goal.
- Retirement creates zero implementation tasks and no runtime code, telemetry,
  cap change, eviction, measurement work, or automated tests.

### Bound any later eviction proposal

As a maintainer, I want any evidence-backed follow-up to preserve lifecycle
truth so reclaiming memory cannot make delayed events appear trustworthy.

Acceptance criteria:

- Passing the evidence gate creates a separate implementation specification;
  this feature does not implement eviction.
- The later specification defines which entries are safe to remove using
  lifecycle epochs and watermarks, not wall-clock age or arbitrary TTL.
- Eviction remains bounded and deterministic and never converts unknown into
  zero or a positive count.
- Delayed, duplicate, and out-of-order events cannot recreate coverage from an
  evicted watermark.
- The later specification includes measured workload frequency, memory impact,
  and user-visible null duration before selecting an eviction policy.
- These criteria create no current backlog item; later work begins only after
  separate human authorization.

## Constraints

- Local source-backed truth and explicit gaps govern every decision under
  constitution principle 1.
- Bounded, nonblocking Rust state remains the owning architecture under
  principles 2 and 3.
- Expected saturation remains display-safe null under principle 5.
- Any future performance or capacity claim requires reproducible measurement
  under principle 10.
- No off-device transmission is permitted under principle 11.
- The specify artifact changes no functionality, architecture, tests, or
  runtime behavior; `lat.md` therefore requires no content update.
- The existing Feature 021 contract remains authoritative: audit persistence is
  non-authoritative, restart clears process-local state, and only qualifying
  root epochs establish coverage.
- Backlog provenance must remain explicit: `quill-9sy` is the source, `quill-k0x`
  is text-only provenance, and the target is the minimum new durable decision
  record.
- Human approval retires source P4 `quill-9sy` as a non-goal. This pipeline
  creates no measurement or implementation tasks.
- If separately authorized later, measurement requires explicit approval, may
  retain only transient minimal local aggregates, and deletes them immediately
  after reporting.
- Any such separately authorized measurement is bounded to one Quill boot and
  stops at process exit or the first root to reach 256 distinct IDs.

## Open Questions

None. The human approved retirement, so measurement authorization and its
operating bounds are not active questions in this pipeline. The conditional
answers below apply only if a separate future authorization reopens them.

## Spec Review

### Critical Questions (answer before planning)

1. Which disposition should planning adopt?
   - **A:** Retire `quill-9sy` now as an evidence-backed non-goal.
   - **B:** Authorize process-scoped measurement of the 256-agent guard as
     separate work.
2. If measurement is authorized, who may initiate and approve it, what local
   lifecycle data may it retain, and when must that data be deleted?
3. If measurement is authorized, what bounded observation window and stop
   criteria determine either demonstrated user impact or another no-go result?

### Non-Blocking Observations

- Requirements consistently distinguish the 256-agent guard from the
  1,024-root registry cap and preserve fail-closed nullable semantics.
- Recorded production evidence supports the stated no-code decision; known
  audit gaps are explicit and correctly excluded from proof.
- Retirement is feasible without runtime, test, UI, database, or `lat.md`
  changes. Measurement and eviction remain separate, unauthorized scopes.
- Maintainers and Quill users are represented. Any measurement follow-up must
  name its operator, data owner, and retirement approver.
- The minimum durable decision record and source-P4 disposition belong to later
  Molecule materialization, not this documentation-only review.

## Clarifications

1. Which disposition should planning adopt?
   - **Answer A:** Retire `quill-9sy` now as an evidence-backed non-goal. No
     measurement work, eviction, cap change, telemetry, runtime code, or
     automated tests are authorized.
2. If measurement is authorized, who may initiate and approve it, what local
   lifecycle data may it retain, and when must that data be deleted?
   - **Answer A:** This question is not active because answer 1 retires
     measurement. If separate future authorization reopens measurement, it
     requires explicit approval, retains only transient minimal local
     aggregates, and deletes them immediately after reporting.
3. If measurement is authorized, what bounded observation window and stop
   criteria determine either demonstrated user impact or another no-go result?
   - **Answer A:** This question is not active because answer 1 retires
     measurement. If separately authorized later, observe one Quill boot and
     stop at process exit or the first root to reach 256 distinct IDs.
