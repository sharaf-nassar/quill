# Plan: observed-agent-cap-saturation

## Architecture Approach

Materialize an artifact-only decision, not a software change. Create one new
durable epic/decision record titled **Validate observed-agent cap saturation**,
record the evidence and explicit human approval, close that record, then close
source P4 `quill-9sy` with disposition `approved-non-goal`. Do not create child
work items or attach the record to closed `quill-k0x`.

The decision preserves the shipped fail-closed design unchanged. The source
marker at `server.rs:211` guards `MAX_OBSERVED_AGENTS_PER_ROOT = 256`, while
`MAX_OBSERVED_ROOTS = 1024` is the separate process-local root registry limit.
Stopped IDs still consume a root epoch's allowance; the 257th distinct ID
invalidates only that root until a strictly newer qualifying `SessionStart`.
A full root registry rejects new roots until process restart.
Existing known roots retain their state, untracked roots remain null, and a
newer epoch cannot allocate a root while the registry is full. Restart clears
process-local state; audit persistence remains non-authoritative, and only a
qualifying root epoch can establish coverage.

The evidence gate is already resolved. The production snapshot at
`2026-08-05T04:16:41.881365Z` used schema 37 and 306,599 hook rows. Eligible
evidence found Claude at 27 rows over 3 roots, maximum 9 distinct IDs and peak
2, and Codex at 48 rows over 12 roots, maximum 7 distinct IDs and peak 3.
Eight eligible roots existed since the latest logged server boot. Neither cap
was saturated, so the result is **not demonstrated**.

Historical and synthetic evidence cannot reverse that result. Audit rows omit
source and boot identity, 4,439 lifecycle rows omit hostname, five audit writes
failed, and persistence follows runtime folding. The apparent 389 Codex IDs,
or at most 67 between `SessionStart` boundaries, cannot establish one root
epoch or process. Existing 257-agent and 1,024-root tests prove boundary
correctness only.

Requirement and clarification trace:

| Requirement | Planned disposition |
| --- | --- |
| Human answer 1A | Record explicit approval and retire `quill-9sy` now as `approved-non-goal`. |
| Human answers 2A and 3A | Inactive under answer 1A; create no measurement work, placeholder, or follow-up. |
| Minimum durable output | Create and close one new epic/decision record with the exact title; create no children. |
| Provenance | Name `quill-9sy` as source; treat `quill-k0x` as text-only history and add no parent or dependency edge to it. |
| No-code result | Leave runtime, caps, folding, nullable semantics, tests, storage, IPC, UI, telemetry, configuration, and `lat.md` behavior unchanged. |
| Backlog result | Close `quill-9sy` during materialization and verify ready P4 count is zero. |
| Future eviction | Outside this pipeline; create no item or placeholder. |

Constitution alignment:

- Principle 1: the decision uses local source-backed evidence, preserves
  explicit gaps, and records saturation as not demonstrated.
- Principle 2: no Rust/Tauri, storage, IPC, React, or ownership boundary
  changes.
- Principle 3: no setup, UI-thread, background, database, network, or other
  runtime work.
- Principle 4: no application mutation or migration; materialization changes
  only durable backlog records and closes them deliberately.
- Principle 5: existing nullable, display-safe saturation behavior and failure
  boundaries remain unchanged.
- Principle 6: no code quality gate becomes applicable; artifact checks remain
  `git diff --check` and `lat check`.
- Principle 7: no automated test code is authorized or added.
- Principle 8: no functionality, architecture, or test behavior changes, so no
  `lat.md` content update is needed; `lat check` still validates consistency.
- Principle 9: no UI change invokes Glass Cockpit requirements.
- Principle 10: no performance or saturation claim is inferred; any future
  capacity work requires separately authorized reproducible measurement.
- Principle 11: no off-device transmission or new data collection occurs.
- Principle 12: the decision remains Beads-tracked, closes the source P4, and
  performs no commit, sync, push, or delivery action without separate authority.

## Affected Components

- `specs/022-observed-agent-cap-saturation/plan.md`: the only repository file
  created by planning.
- New durable epic/decision record **Validate observed-agent cap saturation**:
  receives the cap identity, concrete evidence, limitations, human approval,
  `approved-non-goal` outcome, and then closes without children.
- Source P4 `quill-9sy`: closes during materialization as the approved non-goal
  and cross-references the decision record.
- Closed `quill-k0x`: unchanged; receives no parent, child, dependency, or
  discovered-from edge.

No source, runtime, database, migration, API, IPC, UI, telemetry, test,
configuration, or `lat.md` component is affected.

## Data Model

No application data model, schema, migration, persisted measurement, or local
runtime aggregate is added.

The only durable data is backlog metadata:

- one epic/decision record with the exact target title;
- literal disposition `approved-non-goal`;
- explicit human approval;
- source provenance `quill-9sy`;
- the resolved 256-agent and separate 1,024-root identities;
- the production snapshot values and evidence limitations above;
- a closed status and zero child work items; and
- no relationship to `quill-k0x`.

No measurement parameter, task, placeholder, or dataset is materialized.

## API / Interface Changes

None. No Rust API, Tauri command, HTTP endpoint, event, TypeScript type,
database interface, CLI contract, UI, telemetry, or external transmission
changes. Materialization uses only existing backlog record and closure
operations.

## Testing Strategy

Add no automated tests, fixtures, harnesses, runtime probes, measurements, or
manual UI checks. The outcome has no executable behavior to test.

Artifact verification is limited to confirming:

- exactly one new target epic/decision record exists with the exact title and
  is closed after its evidence-backed decision is recorded;
- its text includes the cap distinction, concrete snapshot evidence, evidence
  limitations, `not demonstrated` conclusion, and explicit human approval;
- it has zero child or P0-P3 implementation work items and no relationship to
  `quill-k0x`;
- `quill-9sy` is closed as `approved-non-goal` and retains traceable provenance
  to the decision record;
- no measurement, runtime, migration, API, UI, telemetry, test, or other
  implementation task exists; and
- ready P4 count is zero after materialization.

Do not inspect or modify the live Quill window or local application data during
verification.

For this planning artifact, run `git diff --check` and `lat check`. No broader
quality gate is justified because no code or behavior changes.

## Risks

- **Wrong cap retired:** conflating 1,024 roots with the source's 256-agent
  marker would preserve a false decision. Mitigation: record both limits and
  the `server.rs:211` marker explicitly.
- **Evidence overstated:** retained audit history or synthetic tests could be
  presented as process-scoped saturation. Mitigation: record every known gap
  and the `not demonstrated` conclusion.
- **Artifact becomes implementation scope:** a decision epic could acquire
  child work. Mitigation: create no P0-P3 items and close it immediately after
  recording the decision.
- **Provenance becomes false hierarchy:** text mentioning `quill-k0x` could be
  mistaken for parentage. Mitigation: leave it unchanged and create no edge.
- **Non-goal scope creep:** conditional measurement or eviction language could
  be mistaken for current work. Mitigation: materialize no task or placeholder.
- **Backlog remains actionable:** leaving `quill-9sy` open would contradict the
  selected disposition. Mitigation: close it during materialization and verify
  ready P4 count is zero.

## Sequencing

1. Create the unparented epic/decision record **Validate observed-agent cap
   saturation** with no children or dependency edges.
2. Record source provenance, both cap identities, current semantics, production
   evidence, evidence limitations, the `not demonstrated` conclusion, explicit
   human approval, and disposition `approved-non-goal`.
3. Close the decision record after its decision is complete.
4. Close source P4 `quill-9sy` as the approved non-goal, referencing the durable
   decision record without attaching either record to `quill-k0x`.
5. Verify the target record is closed with zero children, no P0-P3 work item
   exists, and ready P4 count is zero.

These are materialization steps, not implementation tasks. No parallel work or
dependency graph is needed.

## Backlog Refinement

`quill-9sy` receives final disposition `approved-non-goal` and closes. Its
source marker, cap identity, evidence review, and explicit human approval move
into the one durable decision record. It produces zero implementation,
measurement, migration, API, UI, telemetry, test, or follow-up tasks.

Do not reopen or attach work to `quill-k0x`. Do not create a placeholder for
eviction, measurement, or any speculative follow-up.

Materialization is complete only when both the target decision record and
`quill-9sy` are closed and ready P4 count is zero.

## Target Epic

Create exactly one new durable epic/decision record titled **Validate
observed-agent cap saturation**. Keep it independent of closed `quill-k0x`, add
no child work items, record the evidence-backed `approved-non-goal` decision
and explicit human approval, then close it in the same materialization flow.
This closed tracking artifact is not a P0-P3 implementation work item.

## Alignment fixes applied

- Removed duplicated dormant measurement and eviction details that could read
  as current scope while preserving their explicit non-goal disposition.
- Tightened backlog language to forbid measurement, eviction, and speculative
  placeholders; the output remains one closed decision epic, retired
  `quill-9sy`, and zero P0-P3 implementation tasks.
