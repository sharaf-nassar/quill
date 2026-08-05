# Analysis: observed-agent-cap-saturation

The specification and plan align on an artifact-only, no-code outcome. No
implementation or measurement backlog is warranted.

## Coverage Table

All user stories and acceptance requirements have a complete planned
disposition. “Full” includes explicit deferral where separate future human
authorization is itself the requirement.

| ID | User story / requirement | Plan coverage | Status |
| --- | --- | --- | --- |
| US1 | Identify the actual saturation boundary. | Architecture Approach; Data Model; Target Epic | Full |
| US1.1 | Identify `MAX_OBSERVED_ROOTS = 1024` as the process-local root-entry cap. | Architecture Approach | Full |
| US1.2 | Identify `MAX_OBSERVED_AGENTS_PER_ROOT = 256` as the per-root distinct-lifecycle cap. | Architecture Approach | Full |
| US1.3 | Resolve `quill-9sy` and `server.rs:211` to the 256-agent guard. | Architecture Approach; Risks | Full |
| US1.4 | Record that a full root registry rejects new roots until restart, including newer epochs. | Architecture Approach | Full |
| US1.5 | Record that per-root overflow invalidates only that root until a strictly newer qualifying `SessionStart`. | Architecture Approach | Full |
| US1.6 | Record that stopped IDs still consume the current root epoch allowance. | Architecture Approach | Full |
| US1.7 | Record that the 257th distinct ID invalidates coverage until a strictly newer qualifying epoch. | Architecture Approach | Full |
| US2 | Use existing evidence to reject eviction. | Architecture Approach; Testing Strategy; Risks | Full |
| US2.1 | Preserve snapshot time, schema 37, and 306,599 hook rows. | Architecture Approach; Data Model | Full |
| US2.2 | Preserve Claude and Codex eligible-row, root, distinct-ID, and peak values. | Architecture Approach; Data Model | Full |
| US2.3 | Record eight eligible roots since latest boot and no saturation of either cap. | Architecture Approach | Full |
| US2.4 | Reject unbounded history, synthetic tests, estimates, and extrapolations as saturation proof. | Architecture Approach; Testing Strategy | Full |
| US2.5 | Preserve source, boot, hostname, failed-write, and post-fold persistence limitations. | Architecture Approach; Data Model | Full |
| US2.6 | Reject apparent 389 IDs and at-most-67 boundary-grouped IDs as one root epoch or process. | Architecture Approach | Full |
| US2.7 | Conclude `not demonstrated` without adding collection code. | Architecture Approach; API / Interface Changes | Full |
| US2.8 | Retire `quill-9sy` without measurement, eviction, cap, telemetry, runtime, or test work. | Backlog Refinement; Sequencing | Full |
| US3 | Preserve the safe no-code outcome. | Architecture Approach; Affected Components | Full |
| US3.1 | Leave both caps, `ObservedSubagentState`, and existing tests unchanged. | Architecture Approach; Affected Components | Full |
| US3.2 | Preserve fail-closed known, untracked, and overflow semantics. | Architecture Approach | Full |
| US3.3 | Make no telemetry, database, IPC, UI, configuration, or `lat.md` behavior change. | Affected Components; API / Interface Changes | Full |
| US3.4 | Record human approval to retire `quill-9sy` as an evidence-backed non-goal. | Architecture Approach; Data Model; Backlog Refinement | Full |
| US3.5 | Create zero implementation, runtime, telemetry, cap, eviction, measurement, or test tasks. | Testing Strategy; Backlog Refinement; Target Epic | Full |
| US4 | Bound any later eviction proposal without creating current work. | Architecture Approach requirement trace; Backlog Refinement | Full |
| US4.1 | Require a separate implementation specification after evidence and authorization. | Architecture Approach requirement trace; Backlog Refinement | Full |
| US4.2 | Defer lifecycle-epoch and watermark-safe removal design to that separate specification. | Architecture Approach requirement trace; Backlog Refinement | Full |
| US4.3 | Defer bounded, deterministic, null-preserving eviction design to that separate specification. | Architecture Approach requirement trace; Backlog Refinement | Full |
| US4.4 | Defer delayed, duplicate, and out-of-order event safeguards to that separate specification. | Architecture Approach requirement trace; Backlog Refinement | Full |
| US4.5 | Require separately authorized reproducible workload and impact measurement before policy selection, with no current item. | Constitution alignment; Backlog Refinement | Full |

## Backlog Disposition

| Source | Disposition | Ready |
| --- | --- | --- |
| `quill-9sy` | Artifact-only `approved-non-goal`; close after linking the decision record. | Yes |

The disposition is explicitly human-approved. `quill-k0x` remains text-only
provenance and receives no hierarchy or dependency edge.

## Target Epic

Create exactly one new minimal, unparented epic/decision record titled
**Validate observed-agent cap saturation**. Record the cap distinction,
evidence, limitations, `not demonstrated` result, human approval, and
`approved-non-goal` disposition; then close it with zero children.

The target is unambiguous: one closed decision record, no relationship to
`quill-k0x`, no placeholders, and intentionally zero P0-P3 tasks.

## Constitution Check

| Principle | Result | Basis |
| --- | --- | --- |
| 1. Local source-backed truth | Pass | Uses local evidence, preserves gaps, and reports saturation as not demonstrated. |
| 2. Established stack and boundaries | Pass | Changes no application layer or ownership boundary. |
| 3. Responsive execution | Pass | Adds no runtime, I/O, network, database, UI-thread, or background work. |
| 4. Recoverable mutation | Pass | Limits mutation to deliberate creation and closure of durable backlog records. |
| 5. Typed failure boundaries | Pass | Preserves existing display-safe null and fail-closed behavior. |
| 6. Zero-warning quality gates | Pass | Applies artifact checks only: `git diff --check` and `lat check`. |
| 7. Authorized behavior testing | Pass | Adds no automated test code; none is authorized. |
| 8. Architecture traceability | Pass | Changes no behavior, architecture, or tests; requires `lat check` with no `lat.md` content change. |
| 9. Glass Cockpit discipline | Pass | Makes no UI change. |
| 10. Measured performance | Pass | Makes no saturation claim and gates future capacity work on separate reproducible measurement. |
| 11. Explicit external transmission | Pass | Adds no collection or off-device transmission. |
| 12. Gated delivery | Pass | Keeps disposition in durable tracking and authorizes no commit, sync, push, or release action. |

No tension or violation exists.

## Recommendation

**GO**, limited to materializing and closing **Validate observed-agent cap
saturation**, then retiring `quill-9sy` as the human-approved
`approved-non-goal` and verifying ready P4 count is zero.

This is not approval for measurement, eviction, cap changes, runtime code,
tests, telemetry, placeholders, or follow-up tasks. Zero P0-P3 tasks is an
intentional outcome, not a coverage gap.

## Human Approval

The human approved **GO** for this exact scope: create and close the decision
record, retire `quill-9sy`, and create zero implementation tasks.
