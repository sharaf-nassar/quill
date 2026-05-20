# Feature Specification: Learning System Hardening

**Feature Branch**: `005-learning-system-hardening`
**Created**: 2026-05-17
**Status**: Draft
**Input**: User description: "we want to carefully investigate and implement the prioritized roadmap above to improve and fix our learning system"

## Context

This feature implements the prioritized remediation roadmap produced by the
behavioral-learning system audit (`Stabilize → Standardize → Scale`). The audit
found the learning loop turns raw session activity into **globally-active
standing instructions** for every future assistant session, but does so with:
near-absent privacy redaction, no human review before global activation, no
ground-truth signal (it self-certifies on the generating model's own
confidence), no provenance or rollback, deleted rules that can silently
resurrect, and zero evaluation of whether a rule helps or harms. The goal of
this feature is to make the learning loop **safe, reviewable, evidence-grounded,
reversible, and measurable** without removing its core value (discovering
genuinely useful behavioral patterns).

Each requirement is tagged with the originating audit finding (e.g. `C-1`,
`H-2`) for traceability into planning and tasks.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Sensitive data is protected end to end (Priority: P1)

A developer uses Quill while working on a private codebase. Their tool inputs
and outputs contain API keys, access tokens, database URLs with passwords,
proprietary source, and customer PII. As Quill captures activity and analyzes
it, none of this sensitive data is stored unprotected or sent to any analysis
step in clear form — on any analysis path, not just one.

**Why this priority**: This is the highest-impact, lowest-tolerance risk. A
single leaked credential or PII record is a security/compliance incident. It
must be fixed before any other improvement is trustworthy, because every later
stage consumes this data.

**Independent Test**: Seed activity containing a known corpus of secrets and PII
across every capture path, run a full analysis cycle, and inspect (a) data at
rest and (b) every payload handed to the analysis step. Pass = zero unredacted
sensitive tokens anywhere; the rest of the content remains useful enough to
still produce rules.

**Acceptance Scenarios**:

1. **Given** a tool interaction containing a recognized credential, **When** the
   activity is captured, **Then** the stored record contains the redaction
   placeholder and never the raw secret (`C-1`, redaction at capture).
2. **Given** captured activity that reaches any analysis input (tool-use, git,
   session-history, synthesis, memory optimization), **When** that input is
   assembled, **Then** redaction has been applied to every one of those inputs,
   not only a single path (`C-1`).
3. **Given** a high-entropy secret with no well-known prefix, or PII such as an
   email or customer name, **When** redaction runs, **Then** it is masked
   (`C-1`, entropy + PII detection).
4. **Given** content modified outside the app and re-ingested, **When** it is
   reconciled back into the system, **Then** it passes through the same
   sanitization as every other write path (`H-3`).
5. **Given** an analysis step that spawns an external assistant process,
   **When** that process runs, **Then** it is confined by the best available OS
   mechanism on the platform (or, where none exists, flag-isolated), the
   applied confinement state is recorded, and injected/untrusted captured
   content cannot escape its disposable workspace where OS confinement applies
   (`H-5`).

---

### User Story 2 - No rule goes global without review, and every rule is reversible (Priority: P1)

A developer's session history yields a candidate behavioral rule. Before that
rule can become a standing instruction that shapes every future assistant
session across every project, a human can review and approve it. Any active
rule can be traced to exactly what produced it, restored to a prior version,
and permanently removed so it never comes back on its own.

**Why this priority**: Equal-highest risk. Today a single over-confident
extraction silently rewrites global assistant behavior with no approval, no
history, and no durable delete. Reversibility and review are prerequisites for
safely enabling learning at all.

**Independent Test**: Trigger an extraction that exceeds the confidence bar;
verify no rule becomes globally active without an explicit human approval
action. Approve one, edit it, then roll it back and confirm the prior content
is restored. Delete a rule, run several more analysis cycles, and confirm it
never reappears.

**Acceptance Scenarios**:

1. **Given** a candidate rule above the confidence bar, **When** an analysis
   run completes, **Then** the rule is recorded as awaiting human review and is
   NOT written as a globally-active instruction until a human approves it; no
   autonomous-promotion path exists (`C-2`, Q1).
2. **Given** any rule that has ever been active, **When** an operator inspects
   it, **Then** they can see the run, model, timestamp, and source evidence
   that produced it (`C-2` provenance).
3. **Given** a rule that has been changed at least once, **When** an operator
   chooses to roll back, **Then** a previous version is restored and the change
   is recorded (`C-2` version history + rollback).
4. **Given** a rule a human has deleted or suppressed, **When** later analysis
   cycles re-encounter the same pattern, **Then** the rule does not silently
   return to active status (`C-5` durable tombstone).
5. **Given** a request to promote or delete a rule, **When** it is issued,
   **Then** the request is authorized before it can change on-disk active state
   (`H-4`).

---

### User Story 3 - Rules are promoted on real evidence, not the model's self-rating (Priority: P2)

When the system decides a candidate rule is good enough to act on, that decision
reflects how much real, attributable evidence supports it — not merely how
confident the extracting model claimed to be. A pattern seen once does not
become a permanent global rule; a rule that cites evidence which does not exist
is rejected; contradictory rules are reconciled rather than both kept.

**Why this priority**: Without this, even a perfectly redacted, fully reviewed
pipeline promotes hallucinated or one-off rules. It directly determines rule
quality, but it depends on the safety floor (P1) being in place first.

**Independent Test**: Produce a candidate rule from a single source with minimal
evidence and confirm it is not surfaced as eligible for approval. Produce a
rule whose cited
evidence cannot be matched to real captured activity and confirm it is rejected.
Introduce two conflicting rules and confirm the system flags/reconciles them
instead of activating both.

**Acceptance Scenarios**:

1. **Given** a candidate rule, **When** the promotion decision is made, **Then**
   it uses the evidence-weighted confidence (incorporating supporting/
   contradicting evidence and freshness), not the raw model self-rating
   (`C-3`).
2. **Given** a candidate rule backed by fewer than the minimum evidence
   threshold, **When** review-eligibility is evaluated, **Then** it is not
   surfaced as eligible for approval regardless of stated confidence (`H-2`).
3. **Given** a candidate rule, **When** it is recorded, **Then** it carries
   citations to the specific captured evidence supporting it, and citations
   that cannot be resolved cause the rule to be rejected (`H-1` grounding).
4. **Given** a verdict that an existing rule is irrelevant or contradicted,
   **When** the run applies verdicts, **Then** that verdict measurably affects
   the rule's state instead of being silently discarded (`M-4`).
5. **Given** two candidate rules that conflict or substantially duplicate,
   **When** they are processed, **Then** the system reconciles, merges, or
   flags them rather than activating both independently (`M-3`).

---

### User Story 4 - The system can prove a rule helps before it ships (Priority: P2)

Before trusting the learning loop, a maintainer can run a candidate rule
against a frozen, representative replay set and get a clear verdict on whether
it improves, leaves unchanged, or regresses behavior compared to not having the
rule. Core learning logic is covered by automated tests that run in continuous
integration so regressions are caught before release.

**Why this priority**: This is what lets the loop be trusted and iterated
safely, and is the precondition for trusting the loop and safely reducing
human review burden over time. It is P2 because it builds on the evidence and
governance machinery from P1/P3.

**Independent Test**: Run the evaluation harness for a candidate rule and
confirm it returns a with/without comparison verdict including a
regression/negative-transfer signal. Break a piece of learning logic and
confirm the automated test gate fails in CI.

**Acceptance Scenarios**:

1. **Given** a candidate or active rule, **When** the evaluation harness runs,
   **Then** it produces a paired with-rule vs. without-rule outcome comparison
   over a frozen replay set (`C-4` counterfactual).
2. **Given** a rule that worsens outcomes on the replay set, **When** evaluation
   completes, **Then** the rule is flagged as regressing and cannot be approved
   without an explicit recorded reviewer override (`C-4` negative-transfer).
3. **Given** a change to confidence scoring, state transitions, or synthesis
   decision logic, **When** the test suite runs in CI, **Then** incorrect
   behavior fails the build (`C-4` tests + CI gate).
4. **Given** an evaluation result, **When** it is stored, **Then** it is linked
   to the rule and the run so the verdict is auditable later (`C-4`).

---

### User Story 5 - Operators can see and trust the loop's cost and health (Priority: P3)

A maintainer can open the learning view and see, per analysis run, what it cost,
how long it took, which model was actually used, how many observations it drew
on, and whether any stream failed — and can trust that historical trends and
observation retention behave as documented.

**Why this priority**: Important for long-term operability and cost control, but
the system is still safe and correct without it; it polishes and exposes what
the earlier stories make reliable.

**Independent Test**: Run several analysis cycles and confirm the run history
surfaces accurate cost, latency, model, observation count, and error status for
each. Verify observations are not deleted before they have had a chance to be
analyzed, and that historical summaries are actually used.

**Acceptance Scenarios**:

1. **Given** completed analysis runs, **When** an operator views run history,
   **Then** per-run cost, latency, model, observation count, and error/degraded
   status are displayed for each (`H-6`).
2. **Given** the synthesis step, **When** it runs, **Then** it uses a
   version-pinned model so behavior and attribution do not drift over time
   (`H-7`).
3. **Given** captured observations that have not yet been analyzed, **When**
   retention cleanup runs, **Then** unanalyzed observations are not deleted
   before they can contribute to a run (`M-2`).
4. **Given** historical observation summaries are recorded, **When** the system
   reports trends, **Then** those summaries are actually read and used, not
   written to a dead end (`M-1`).
5. **Given** both supported providers, **When** activity is captured, **Then**
   coverage asymmetry between providers is reduced or explicitly disclosed so
   shared rules are not silently biased (`M-6`).

### Edge Cases

- A secret is split across a truncation/compression boundary so no single
  fragment matches a redaction pattern — what is the masking guarantee? (`C-1`)
- An analysis run produces zero usable findings, or one stream fails while
  others succeed — the run status must clearly distinguish completed, degraded,
  and failed, and must not write partial/unsafe rules.
- A rule is approved, then the underlying evidence is later purged by retention
  — provenance must still resolve to at least the run/model/citation summary,
  even if raw evidence is gone (`C-2` vs retention interaction).
- A human edits an active rule file directly on disk while the app is running —
  the change must be sanitized, versioned, and attributable, not trusted
  verbatim (`H-3`).
- An operator deletes a rule that is simultaneously re-extracted with strong new
  evidence in the same cycle — the durable-tombstone decision must be
  deterministic and explainable (`C-5`).
- Pre-existing rules written by the old unsafe path are archived to a one-time
  read-only backup and removed from active use; none remain active, and they
  return only by being re-discovered through the new gated pipeline (`C-2`, Q3).
- A candidate is awaiting review when a later run re-derives the same rule with
  different content — the review queue must not duplicate the entry or silently
  overwrite the pending version; the reviewer must see that it changed.
- The evaluation replay set drifts out of date relative to current assistant
  behavior — staleness of the frozen baseline must be detectable.

## Requirements *(mandatory)*

### Functional Requirements

**Privacy & data protection (P1 / C-1, H-3, H-5)**

- **FR-001**: The system MUST redact recognized secrets and PII at capture time,
  before any sensitive value is persisted at rest (`C-1`).
- **FR-002**: The system MUST apply redaction to every input consumed by any
  analysis or synthesis step, with no analysis path exempt (`C-1`).
- **FR-003**: Redaction MUST detect, at minimum: well-known credential formats,
  high-entropy secrets without a known prefix, credentials embedded in
  connection strings/URLs, and PII categories including email addresses and
  personal/customer names; and MUST run before any lossy compression so
  boundary fragmentation does not defeat it (`C-1`).
- **FR-004**: Content re-entering the system from outside the application MUST
  pass through the same sanitization as internally generated writes (`H-3`).
- **FR-005**: Any externally spawned analysis process MUST be confined using
  the best available OS-level mechanism on the host platform so that untrusted
  captured content it processes cannot read or modify data outside a
  disposable, isolated workspace, and the applied confinement state MUST be
  recorded per run. Where a platform provides no usable OS confinement (e.g.
  some Windows configurations), the process runs with the existing flag-based
  isolation and the reduced confinement is recorded and disclosed; the system
  MUST NOT fail closed (disable learning) for lack of OS confinement (`H-5`).
- **FR-006**: Redaction MUST preserve enough non-sensitive structure that
  legitimate patterns can still be learned (redaction is not blanket deletion).

**Governance, provenance & reversibility (P1 / C-2, C-5, H-4)**

- **FR-007**: A behavioral rule MUST NOT become a globally-active instruction
  without an explicit human approval action. The system MUST NOT provide any
  autonomous (no-human) promotion path (`C-2`, Q1 = remove autonomous
  promotion entirely).
- **FR-008**: Every rule that has ever been active MUST carry resolvable
  provenance: the originating run, the model used, a timestamp, and references
  to the supporting evidence (`C-2`).
- **FR-009**: The system MUST retain version history for rule content and MUST
  support rolling a rule back to a prior version, recording the rollback
  (`C-2`).
- **FR-010**: A rule that a human deletes or suppresses MUST NOT silently return
  to active status in any later analysis cycle; reactivation MUST require an
  explicit human action (`C-5`).
- **FR-011**: Requests that change a rule's active on-disk state (promote,
  delete, edit) MUST be authorized before taking effect (`H-4`).
- **FR-012**: Rules already on disk lacking provenance MUST be archived to a
  one-time read-only backup and then removed from active use; they MUST NOT
  remain active and MUST return only by being re-discovered through the new
  gated pipeline (`C-2`, Q3 = wipe-and-relearn with archive).
- **FR-013**: Run status MUST clearly distinguish completed, degraded
  (some streams failed), and failed; degraded/failed runs MUST NOT write
  partial or unsafe rules.

**Evidence-grounded promotion (P2 / C-3, H-1, H-2, M-3, M-4)**

- **FR-014**: The decision of which candidates are surfaced as eligible for
  human approval MUST use an evidence-weighted confidence that incorporates
  supporting/contradicting evidence and freshness, not the raw extracting-model
  self-rating (`C-3`).
- **FR-015**: A candidate rule MUST cite the specific captured evidence
  supporting it; rules whose citations cannot be resolved to real evidence MUST
  be rejected (`H-1`).
- **FR-016**: A candidate rule MUST meet a minimum evidence threshold (count
  and/or distinct sources) before it is eligible to be surfaced for approval,
  regardless of stated confidence (`H-2`).
- **FR-017**: Verdicts on existing rules (including "irrelevant" /
  "contradicted") MUST measurably affect rule state and MUST NOT be silently
  discarded (`M-4`).
- **FR-018**: Conflicting or substantially duplicate candidate rules MUST be
  reconciled, merged, or flagged rather than independently activated (`M-3`).

**Evaluation & regression safety (P2 / C-4)**

- **FR-019**: The system MUST provide an evaluation capability that compares
  behavior with vs. without a given rule over a frozen, representative replay
  set (`C-4`).
- **FR-020**: Evaluation MUST surface a regression / negative-transfer signal,
  and a rule that regresses the replay set MUST be blocked from approval unless
  a reviewer records an explicit override decision (`C-4`).
- **FR-021**: Core learning logic (confidence scoring, state transitions,
  promotion gating, synthesis decision, suppression durability) MUST be covered
  by automated tests executed in continuous integration as a release gate
  (`C-4`).
- **FR-022**: Evaluation results MUST be persisted and linked to the rule and
  run they assess (`C-4`).
- **FR-023**: The system MUST detect and disclose when the frozen replay set is
  stale relative to current behavior (`C-4`).

**Observability & operational correctness (P3 / H-6, H-7, M-1, M-2, M-6)**

- **FR-024**: Per-run cost, latency, model actually used, observation count, and
  error/degraded status MUST be readable and surfaced to operators for every
  run (`H-6`).
- **FR-025**: The synthesis step MUST use a version-pinned model (`H-7`).
- **FR-026**: Retention cleanup MUST NOT delete observations that have not yet
  had the opportunity to be analyzed by at least one run (`M-2`).
- **FR-027**: Recorded historical summaries MUST be consumed by the features
  that report trends, or be removed if unused (`M-1`).
- **FR-028**: Cross-provider capture asymmetry MUST be reduced, or explicitly
  disclosed wherever provider-shared rules are presented (`M-6`).

**Outcome signal (cross-cutting, gates C-3/C-4)**

- **FR-029**: The system MUST provide a per-rule operator control on rules in
  the Learning UI with exactly three actions — accept, reject, and "this rule
  was bad" (`bad` distinct from `reject`); this feedback MUST
  be persisted and used as the primary outcome signal for evidence weighting
  (`C-3`) and evaluation (`C-4`). This is rule-level operator feedback, not
  end-user satisfaction telemetry, and remains consistent with the prior
  feature's exclusion of satisfaction metrics (`C-3`, Q2 = explicit operator
  feedback).

### Sequencing & Dependencies

- P1 stories (US1, US2) are the stabilization floor and MUST land before any
  rule is trusted as active or any later story builds on them; there is no
  autonomous-promotion milestone (Q1 = human approval always).
- US3 depends on FR-029 (outcome signal, Q2) for true evidence weighting.
- US4 depends on US3's evidence/grounding and on FR-029.
- US5 is independent and may proceed in parallel but is lowest priority.

### Key Entities *(include if feature involves data)*

- **Observation**: A captured unit of assistant activity. Must support
  redaction-at-capture and resolvable linkage to its source.
- **Learned Rule**: A behavioral instruction candidate or active rule. Gains
  provenance, version history, evidence citations, lifecycle state, and a
  durable suppression/tombstone state.
- **Rule Version**: A historical snapshot of a rule's content enabling rollback
  and change auditing.
- **Provenance Record**: The link from a rule to the run, model, timestamp, and
  evidence that produced it; must remain resolvable even after raw evidence is
  purged.
- **Analysis Run**: One execution of the learning pipeline, with status
  (completed/degraded/failed), cost/latency/model telemetry, and outcome.
- **Evidence Citation**: A resolvable reference from a candidate rule to the
  specific captured activity supporting it.
- **Promotion Decision / Approval**: The record of why a rule was (not)
  surfaced for review, the evidence-weighted score used, and the human actor
  who approved or rejected it.
- **Suppression Tombstone**: A durable marker that a rule was deliberately
  removed and must not auto-reactivate.
- **Evaluation Result**: A with/without comparison verdict for a rule over the
  replay set, including a regression signal, linked to rule and run.
- **Replay Set**: The frozen, representative corpus used for counterfactual
  evaluation, with a staleness indicator.
- **Outcome Signal**: Operator accept/reject (and "this rule was bad")
  feedback on rules, persisted and used as the primary input for evidence
  weighting and evaluation.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Against a known seeded corpus of secrets and PII exercised across
  every capture path, zero unredacted sensitive tokens appear at rest or in any
  analysis input (0 leaks).
- **SC-002**: 0% of rules become globally-active without an explicit human
  approval action (no autonomous-promotion path exists).
- **SC-003**: 100% of rules that have ever been active can be traced to the run,
  model, timestamp, and evidence that produced them.
- **SC-004**: 100% of active rules can be rolled back to a prior version (when
  one exists) and 100% of human-deleted rules remain inactive across at least 5
  subsequent analysis cycles (0 resurrections).
- **SC-005**: 0% of rules backed by fewer than the minimum evidence threshold,
  or with unresolvable evidence citations, are surfaced as eligible for
  approval or activated.
- **SC-006**: 100% of review-eligibility decisions (which candidates are
  surfaced for approval) use the evidence-weighted score; no decision uses the
  raw model self-rating alone.
- **SC-007**: For 100% of candidate rules submitted to evaluation, the system
  returns a with/without verdict including a regression signal; 100% of rules
  approved for activation have a non-regressing verdict available at approval
  time (or a recorded reviewer override).
- **SC-008**: Core learning logic has automated test coverage that runs as a CI
  release gate; introducing a known defect in scoring/state/promotion logic
  fails CI in 100% of trials.
- **SC-009**: Operators can view accurate per-run cost, latency, model,
  observation count, and status for 100% of analysis runs.
- **SC-010**: 0 observations are deleted by retention before having had at least
  one opportunity to be analyzed.
- **SC-011**: The end-to-end remediation does not reduce the count or
  reviewer-judged usefulness of genuinely useful discovered rules below the
  pre-remediation baseline (learning value is preserved).
- **SC-012**: After migration, 0 active rules lack provenance, and 100% of
  archived legacy rules are recoverable from the one-time read-only archive
  (the migration-time specialization of SC-003).
- **SC-013**: On platforms providing OS confinement, the spawned analysis
  process cannot read or modify data outside its per-call disposable workspace
  (verified by an attempted out-of-workspace access), and the applied
  confinement state is recorded for 100% of analysis runs on every platform
  (`FR-005`).

## Assumptions

- The learning system remains a local-first capability; redaction and isolation
  are enforced on the local machine, not delegated to a remote service.
- The existing audit (`Stabilize → Standardize → Scale` roadmap and findings
  C-1…C-5, H-1…H-7, M-1…M-6, L-1…L-3) is the authoritative input; finding IDs
  are used purely for traceability.
- "Globally-active" means a rule that, once written, influences every future
  assistant session across every project (current behavior); no per-project
  scoping is introduced by this feature unless a clarification changes that.
- The Memory Optimizer subsystem is in scope only where it shares the redaction
  gap (FR-002); its internal suggestion lifecycle is otherwise unchanged.
- Reviewer-judged usefulness (SC-011) is assessed by a maintainer against a
  representative sample, consistent with the prior feature's baseline method;
  the SC-011 comparison baseline (rule counts + reviewer-usefulness sample) is
  captured before any pipeline change lands (see the baseline task in
  tasks.md).
- Automated tests are authored as an explicit requirement of this feature
  (FR-021), overriding the general "tests only on explicit request" policy for
  the learning-logic surface.
- A one-time read-only archive of legacy rules is retained for recovery/audit
  after they are removed from active use (Q3).
- Existing already-captured data at rest (observations, cached git snapshots)
  is redacted by a one-time backfill so FR-001/SC-001 hold for pre-existing
  rows, not only newly captured data (`C-1`).
- Operator accept/reject feedback (Q2) is rule-level maintainer judgment
  surfaced in the existing Learning UI, not new end-user telemetry.

## Clarifications

### Session 2026-05-17

- **Q1 — Autonomous promotion policy (FR-007)** → **Resolved: A.** Human
  approval is ALWAYS required; autonomous (no-human) promotion is removed
  entirely. There is no autonomous-promotion mode or milestone.
- **Q2 — Outcome/feedback signal (FR-029)** → **Resolved: B.** Add a per-rule
  operator control with three actions — accept, reject, and "this rule was
  bad" (`bad` distinct from `reject`) — on rules in the Learning UI as the
  primary outcome signal. Rule-level maintainer feedback, not end-user
  satisfaction telemetry.
- **Q3 — Existing on-disk rules (FR-012)** → **Resolved: C.** Archive legacy
  rules to a one-time read-only backup, then delete them from active use;
  rebuild only through the new gated pipeline (wipe-and-relearn).
