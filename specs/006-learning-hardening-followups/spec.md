# Feature Specification: Learning System Hardening Follow-ups

**Feature Branch**: `006-learning-hardening-followups`
**Created**: 2026-05-18
**Status**: Draft
**Input**: Two deferred follow-ups from the feature-005 "Learning System Hardening" code review (Medium severity each), intentionally excluded from the remediation pass because each needs a real design rather than a rushed inline patch.

## Overview

Feature 005 ("harden learning loop — privacy, governance, eval, CI", commit `1747f9f`) shipped with two known, deferred defects on the CI-gated learning-logic surface. This feature closes both. They are independent and small; they share this spec, plan, and task list.

- **Follow-up A — Misrepresented inference confinement (security/privacy).** On Linux, when `bwrap` is absent, the spawned analysis process is wrapped only in process namespaces with **no filesystem confinement**, yet the run is recorded with a confinement label that implies real filesystem isolation. The recorded/disclosed state overstates the actual protection.
- **Follow-up B — Non-atomic rule version vs. evidence citations (correctness).** Re-deriving a pending (`awaiting_review`) learned rule with refined content advances the rule's recorded version before that version's supporting evidence is written, creating a window — transient under normal flow, persistent on citation-write failure — in which a previously review-eligible rule has zero evidence at its current version and silently drops out of the review queue.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Inference confinement is reported honestly (Priority: P1)

As the operator running Quill on my own machine, when the learning loop spawns an analysis process to read untrusted captured session content, I need the recorded and displayed confinement state to accurately reflect whether my home directory, credentials, project trees, and database were actually protected — so I can trust the privacy guarantee and take action when the strong protection is unavailable.

**Why this priority**: The defect is a misrepresentation of a privacy/security control. An operator (or auditor) reading the run record or run-history UI on a `bwrap`-absent host currently believes filesystem confinement was applied when it was not. Correcting a false safety signal is higher-stakes than the correctness flicker in Story 2, and is independently shippable.

**Independent Test**: On a host with no filesystem-confinement mechanism available, trigger an analysis run and inspect (a) the persisted per-run confinement state and (b) the run-history UI. The run is unambiguously marked as "no filesystem confinement", visually distinguished from filesystem-confined runs, and accompanied by a remediation hint. Learning still completes (never fail-closed).

**Acceptance Scenarios**:

1. **Given** a Linux host where the strong filesystem-confinement mechanism is unavailable, **When** an analysis run executes, **Then** the persisted per-run confinement state does NOT claim filesystem confinement and the run completes successfully (learning is not disabled).
2. **Given** a completed run that executed without filesystem confinement, **When** the operator views run history, **Then** that run is visibly distinguished from filesystem-confined runs and shows a hint describing how to obtain full confinement.
3. **Given** a host where the strong filesystem-confinement mechanism IS available, **When** an analysis run executes, **Then** the recorded and displayed state reflects full filesystem confinement, unchanged from feature 005 behavior.
4. **Given** any analysis run on any platform, **When** the run record is written (success or failure path), **Then** a confinement state is present for 100% of runs (the feature-005 SC-013 invariant is preserved).

---

### User Story 2 - Review-eligible rules stay in the review queue across re-derivation (Priority: P2)

As the human reviewer of learned rules, when a rule that has already earned review eligibility is re-derived with refined wording from new evidence, I need it to remain visible in my review queue — not silently disappear because the system advanced the rule's version ahead of its supporting evidence.

**Why this priority**: A previously surfaced rule dropping out of the review queue is a correctness/UX regression that erodes trust in the governance pipeline, but it is lower-stakes than a misrepresented security control and is independently shippable.

**Independent Test**: Seed a pending (`awaiting_review`) rule that meets the evidence threshold. Re-derive it with changed content (so its version would advance). Verify the rule never becomes review-ineligible due to a missing evidence snapshot at its recorded version — including the case where the evidence re-write step fails after the version advances.

**Acceptance Scenarios**:

1. **Given** a pending rule that is review-eligible at version N, **When** it is re-derived with changed content, **Then** the rule remains review-eligible throughout and after the operation (no demotion to non-eligible / `candidate`).
2. **Given** a pending rule being re-derived, **When** the evidence-citation re-write step fails after the rule's version would advance, **Then** the rule's recorded version still resolves to a version that has its supporting evidence (the rule does not become permanently un-reviewable).
3. **Given** a pending rule re-derived with **unchanged** content, **When** the operation runs, **Then** behavior is unchanged from feature 005 (no version advance, no eligibility change).
4. **Given** the rule-governance lifecycle (`candidate → awaiting_review → active`, plus terminal states), **When** this fix is applied, **Then** all feature-005 lifecycle, suppression-stickiness, and tombstone behaviors are preserved.

---

### Edge Cases

**Follow-up A:**

- The strong confinement mechanism is present at process-start probe but the process-namespace fallback binary is also absent → the run is recorded as fully unconfined (already honest today) and learning continues.
- The resolved analysis binary/runtime lives under the operator's home directory on a host with no filesystem confinement → the exposure is real and MUST be reflected honestly in the recorded state and UI (not labeled as confined).
- A run fails before the analysis process is spawned → the confinement state is still recorded (SC-013 invariant), and it reflects the host's actual capability without overstating it.

**Follow-up B:**

- The evidence re-write step fails or is interrupted mid-operation → the rule's recorded version never points at a version lacking its evidence; the rule stays reviewable on its last good evidence snapshot.
- A concurrent or later read of review eligibility occurs around the re-derivation → it never observes a "version advanced but evidence missing" state for that rule.
- A re-derived rule that is suppressed or tombstoned → suppression-stickiness from feature 005 is preserved (content/version are not advanced for frozen rules), so the defect does not apply.
- A re-derived rule transitions `awaiting_review → active` around the same time → governance ordering and the single-writer approval path from feature 005 are preserved.

## Requirements *(mandatory)*

### Functional Requirements

**Follow-up A — confinement honesty & disclosure:**

- **FR-001**: The per-run confinement state MUST distinguish "filesystem-confined" from "not filesystem-confined (process/flag isolation only)" so that a recorded state never implies stronger protection than was applied.
- **FR-002**: On a Linux host where the strong filesystem-confinement mechanism is unavailable, the system MUST NOT record or present that run as having filesystem confinement.
- **FR-003**: The operator MUST be able to see, in the run-history view, when a run executed without filesystem confinement, visually distinguished from filesystem-confined runs, with a remediation hint for obtaining full confinement.
- **FR-004**: The system MUST NOT fail closed (MUST NOT disable or skip learning) for lack of OS-level confinement; the loop continues and the reduced state is recorded and disclosed (preserves feature-005 `H-5` / `FR-005`).
- **FR-005**: A confinement state MUST be recorded for 100% of analysis runs on every platform, on both the success and failure paths (preserves feature-005 `SC-013`).
- **FR-006**: Behavior on hosts where the strong filesystem-confinement mechanism IS available MUST be unchanged from feature 005 (no regression to the confinement-applied path).
- **FR-007**: macOS and Windows confinement behavior is out of scope and MUST remain unchanged (the Linux development host cannot compile the macOS configuration path; feature-005 FIX #3 already scoped macOS reads).

**Follow-up B — version/evidence atomicity:**

- **FR-008**: Re-deriving a pending (`awaiting_review`) rule with changed content MUST NOT cause the rule to leave the review-eligible set due to its recorded version having no resolvable supporting evidence.
- **FR-009**: At all times, a rule's recorded current version MUST resolve to evidence sufficient to evaluate review eligibility, OR the version MUST NOT have been advanced — i.e. the version advance and the supporting-evidence (re)write MUST be effectively atomic with respect to any reader of review eligibility.
- **FR-010**: If the supporting-evidence (re)write fails during re-derivation, the rule MUST NOT be left permanently un-reviewable; its recorded version MUST continue to resolve to a version that has its evidence.
- **FR-011**: Re-derivation with unchanged content MUST NOT advance the rule version or alter eligibility (unchanged from feature 005).
- **FR-012**: All feature-005 rule-governance behaviors — lifecycle states and transitions, suppression-stickiness, tombstone gating, the single-writer approval path, the pending-change marker semantics — MUST be preserved.
- **FR-013**: The fix MUST NOT introduce a new database schema migration unless a new additive migration is explicitly justified and documented; the feature-005 data-model constraint (the pending-change marker is the version bump; no dedicated pending-change column exists) MUST be honored or its change justified.

**Both follow-ups — verification surface:**

- **FR-014**: New behavior for both follow-ups MUST be covered by deterministic automated tests on the CI-gated learning-logic surface (preserves feature-005 `FR-021`), using the existing temp-database, serialized-execution, and offline-inference test harness; no test may require a live external analysis process or network.
- **FR-015**: The project knowledge base (`lat.md/`) MUST be updated to reflect the corrected confinement semantics and the corrected version/evidence ordering, and link validation MUST pass.

### Key Entities *(include if feature involves data)*

- **Run Confinement Record**: The per-analysis-run state describing the OS-level confinement actually applied. Must carry enough fidelity to distinguish filesystem-confined from process/flag-only, and is consumed by both the persisted run record and the run-history UI.
- **Learned Rule Version**: The monotonically advancing marker on a pending rule that signals "pending content has changed since it entered review". Constrained by the feature-005 data model: the marker is the version bump itself; no dedicated pending-change column exists.
- **Evidence Citation Snapshot**: The retention-proof set of resolved evidence references attached to a rule at a specific rule version; the review-eligibility gate counts distinct references and sources from this snapshot.
- **Review Eligibility**: The derived gate that promotes a `candidate` to `awaiting_review` (evidence-weighted score ≥ threshold AND distinct references ≥ 3 AND distinct sources ≥ 1 AND not invalidated AND not tombstoned). This feature must keep that gate stable across re-derivation.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Across the deterministic test matrix, 0% of analysis runs on a host without filesystem confinement are recorded or displayed with a state that implies filesystem confinement.
- **SC-002**: A run that executed without filesystem confinement is visually distinguishable in run history and presents a remediation hint, verified by the run-history rendering test/inspection.
- **SC-003**: 100% of analysis runs (success and failure paths, every platform) carry a recorded confinement state — the feature-005 SC-013 invariant is preserved and still passes its existing test.
- **SC-004**: Learning is never disabled for lack of OS confinement — 0 fail-closed events in the test matrix; the loop completes on a fully-unconfined host.
- **SC-005**: Re-deriving a review-eligible pending rule with changed content results in 0 spurious demotions out of review eligibility across the deterministic test matrix, including the simulated evidence-write-failure case.
- **SC-006**: After a simulated evidence-write failure during re-derivation, the rule's recorded version resolves to evidence sufficient for the eligibility gate in 100% of test cases (no permanently un-reviewable rule).
- **SC-007**: No new database migration is introduced, OR exactly one documented additive migration is introduced with a written justification that reconciles the feature-005 data model; schema-version expectations in existing tests still hold.
- **SC-008**: The full feature-005 verification baseline still passes with zero new warnings: format check, a forced clean lint with warnings denied, the library test suite, and the frontend build; `lat check` passes.

## Assumptions

- **Platform scope**: Implementation and verification target the Linux development host. The macOS confinement configuration path cannot be compiled here and is explicitly out of scope; Windows is out of scope. Cross-platform recording invariants (SC-013) are preserved but not re-designed.
- **Never fail-closed**: The feature-005 decision (R-7 / `H-5`) that learning continues even with no OS confinement is retained. This feature corrects honesty/disclosure and may strengthen confinement, but does not introduce a fail-closed mode.
- **Process-namespace fallback retained**: The existing process-level (PID/IPC/UTS) namespace wrapper on the `bwrap`-absent Linux path is assumed worth keeping as defense-in-depth for process isolation; the correction is to stop representing it as filesystem confinement, not necessarily to remove it.
- **Hand-rolled filesystem namespace is high-risk**: The feature-005 research (R-7 "Alternatives") explicitly rejected hand-rolled mount/user namespaces as the primary mechanism (heavy/error-prone). Any option that re-introduces a hand-rolled filesystem sandbox carries that risk and the never-fail-closed constraint; the design phase will weigh this against an honest-disclosure-only approach.
- **No migration preferred for Follow-up B**: The feature-005 data-model "as-built reconciliation" states the pending-change marker is the version bump and there is no pending-change column. The design will prefer an approach needing no migration; introducing one requires explicit justification (FR-013).
- **Test harness reused**: New tests use the existing temp-database + serialized-execution harness, and the existing offline scripted-inference double (with its RAII teardown guard) for the inference path. No live analysis process or network in tests (FR-014).
- **Concurrency model**: The current single-connection, serialized storage access model is assumed unchanged. Follow-up B's fix targets correctness of the ordering/atomicity itself (so the invariant holds regardless of reader timing or write failure), not a new concurrency model.
- **Combined feature**: The two follow-ups are independent but small and share one spec/plan/task list; they can be implemented and verified independently and in either order.
- **Design recommendation deferred to plan**: This specification states required outcomes and is implementation-agnostic. The 2–3 design options per follow-up, the recommended option with tradeoffs, the detailed test strategy, the `lat.md` sync points, and the dependency-ordered task list are produced in the planning phase and presented for approval before any implementation.
