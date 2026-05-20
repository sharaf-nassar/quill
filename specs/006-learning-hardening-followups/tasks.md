---
description: "Task list for Learning System Hardening Follow-ups"
---

# Tasks: Learning System Hardening Follow-ups

**Input**: Design documents from `/specs/006-learning-hardening-followups/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Test tasks are INCLUDED — explicitly required by spec FR-014/FR-021 (CI-gated learning surface; deterministic unit tests on the existing harness).

**Organization**: Grouped by user story. US1 = Follow-up A (confinement honesty, P1). US2 = Follow-up B (version/evidence atomicity, P2). The two stories touch **disjoint files** and are fully parallelizable as two implementation tracks; the integrated post-join 0-warning baseline is authoritative.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: US1 (Follow-up A) or US2 (Follow-up B)

## Path Conventions

Desktop app: Rust backend `src-tauri/src/`, React frontend `src/`, knowledge graph `lat.md/`, specs `specs/006-learning-hardening-followups/`.

---

## Phase 1: Setup (Shared)

**Purpose**: Confirm a clean starting baseline so the 0-warning gate is meaningful.

- [ ] T001 Confirm branch `006-learning-hardening-followups` is checked out and the working tree contains only the `specs/006-*`, `CLAUDE.md`, and `.specify/feature.json` planning changes
- [ ] T002 Establish the pre-change green baseline from repo root: `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`; `npm run build`; `lat check` — record that all pass before any code change

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Verify the shared deterministic test seams exist before writing story tests. No cross-story code dependency (US1 and US2 are independent).

- [ ] T003 [P] Confirm `src-tauri/src/cc_client.rs` exposes the `#[cfg(test)]` `set_inference_double_scoped` / `InferenceDoubleGuard` seam and the `sandbox_metadata_is_recorded_for_every_call` test exists (US1 test substrate)
- [ ] T004 [P] Confirm `src-tauri/src/storage.rs` test substrate (`init_storage_in`, `TempDir`, `#[serial]`, `clear_env()`) used by `store_learned_rule_on_conflict_is_suppression_sticky` and `eligible_for_review_enforces_min_cluster_uniformly_across_streams` (US2 test substrate)

**Checkpoint**: Both substrates confirmed → US1 and US2 may proceed in parallel.

---

## Phase 3: User Story 1 — Inference confinement reported honestly (Priority: P1)

**Goal**: The bwrap-absent Linux path is recorded under an honest tag (process/namespace only, NO filesystem confinement) and surfaced to the operator in run history with a remediation hint; FS-confined behavior unchanged; never fail-closed; SC-013 100%-recorded invariant preserved.

**Independent Test**: On a host with no FS-confinement mechanism, an analysis run completes, the persisted `sandbox` tag is the honest non-FS tag, and run history visually distinguishes it with an "install bwrap" hint.

### Implementation (US1)

- [ ] T005 [US1] In `src-tauri/src/cc_client.rs` add `SandboxKind::ProcessOnly` variant and an `is_fs_confined(self) -> bool` method (true for `Bwrap`/`SandboxExec`; false for `ProcessOnly`/`JobObject`/`None`); extend `as_str()` with a distinct stable tag (e.g. `"process-only"`); rewrite the enum doc so the new variant explicitly states "process/namespace isolation only, NO filesystem confinement"
- [ ] T006 [US1] In `src-tauri/src/cc_client.rs` `detect_sandbox_kind` + `apply_sandbox`: on Linux when `bwrap` is absent, record `SandboxKind::ProcessOnly` (keep the existing `unshare --mount --pid --ipc --uts --fork --kill-child` wrapper running as non-FS process-isolation defense-in-depth, but it no longer claims FS confinement); `bwrap` path and macOS/Windows cfg paths unchanged
- [ ] T007 [US1] In `src-tauri/src/cc_client.rs` update the `InferenceCallMetadata.sandbox` doc comment to state the tag distinguishes filesystem-confined from process/flag-only; confirm `metadata_from_envelope`, `failed_metadata`, and `doubled_metadata` all emit the honest tag (no logic change beyond the new variant flowing through)
- [ ] T008 [US1] In `src/types.ts` add an additive optional `confinement` descriptor (recorded tag + derived `fs_confined: boolean`) to `RunInferenceCall`/`RunInferenceSummary`; update the backend run-inference summary projection (Tauri command/serializer feeding these types) to populate it from the recorded `sandbox` tag
- [ ] T009 [US1] In `src/components/learning/RunHistory.tsx` surface confinement: when a run is not FS-confined, render a distinct marker + remediation hint ("No filesystem confinement on this host — install bwrap for full isolation"); FS-confined runs render unchanged from feature 005

### Tests (US1)

- [ ] T010 [P] [US1] Add a pure mapping test in `src-tauri/src/cc_client.rs` (`#[cfg(test)]`, no spawn, no `#[serial]`): for every `SandboxKind`, assert `as_str()` ∈ closed set and `is_fs_confined()` matches the FS/non-FS table
- [ ] T011 [US1] Adjust `sandbox_metadata_is_recorded_for_every_call` in `src-tauri/src/cc_client.rs`: extend the closed set + Linux `platform_expected` with the new tag; assert `as_str()` round-trips `ProcessOnly`; keep host-agnostic assertions (membership/classification only — never assert the host's actual mechanism)
- [ ] T012 [US1] Add a `RunHistory` render assertion (existing `src/` frontend test infra) — a not-FS-confined run shows the marker + hint; an FS-confined run does not

### lat.md sync (US1)

- [ ] T013 [US1] Update `lat.md/backend.md` "Claude Code Inference Client" (the sandbox + metadata paragraphs ~backend.md:355-368): bwrap-absent Linux = process-namespace isolation only, NO filesystem confinement, recorded under the honest tag; the UI now surfaces it with a remediation hint; update the `[[src-tauri/src/cc_client.rs#SandboxKind]]` reference; run `lat check`

**Checkpoint US1**: Per-track verify — `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`; `npm run build`; `lat check` all green for the US1 changes in isolation.

---

## Phase 4: User Story 2 — Review-eligible rules stay queued across re-derivation (Priority: P2)

**Goal**: `current_version` advances only after the new version's `rule_evidence_citations` are persisted, so the "current_version always resolves to a version with its evidence" invariant holds by construction; closes the transient and persistent (citation-write-failure) windows; no migration; merge-always + suppression-sticky preserved.

**Independent Test**: A v1 review-eligible pending rule re-derived with changed content stays eligible; `current_version` is v2 only after v2 citations exist; a forced citation-write failure leaves `current_version` un-advanced and the rule still eligible on its prior snapshot.

### Implementation (US2)

- [ ] T014 [US2] In `src-tauri/src/storage.rs` `store_learned_rule`: remove the `current_version = CASE WHEN … THEN current_version+1 …` bump from the `ON CONFLICT(name)` clause; preserve the suppression-sticky `file_path`/`content` CASE expressions byte-for-byte; surface a "pending content changed for an `awaiting_review` rule" signal to the caller (return value / out-param computed from old-vs-new content + lifecycle)
- [ ] T015 [US2] In `src-tauri/src/storage.rs` add an atomic post-citation version advance: persist the new-version `rule_evidence_citations` at `target = current_version + 1` and, in the same transaction (or an immediately-following guarded `UPDATE … SET current_version = ?`), advance `current_version` to `target` only after the snapshot rows exist; on citation failure do NOT advance (rule stays at the prior cited version). Adjust `persist_evidence_citations` to write at the target version rather than reading `current_version` before its tx
- [ ] T016 [US2] In `src-tauri/src/learning.rs` `write_rule_files`: rewire the per-rule ordering to use the new flow (`store_learned_rule` merge + pending-changed signal → persist citations at target → atomic bump), preserving the existing non-blocking error logging and the subsequent `eligible_for_review` / `set_rule_lifecycle_if` calls; `eligible_for_review` query is unchanged (it now always reads a cited version)

### Tests (US2)

- [ ] T017 [P] [US2] Add `#[test] #[serial]` test in `src-tauri/src/storage.rs` (TempDir/`init_storage_in`): seed an `awaiting_review` rule at v1 with ≥3 distinct refs + ≥1 source (eligible); re-derive with changed content; assert eligibility stays true and `current_version` becomes v2 only after v2 citations exist (no observable 0-ref current_version)
- [ ] T018 [P] [US2] Add `#[test] #[serial]` citation-failure-injection test in `src-tauri/src/storage.rs`: force the citation step to fail on re-derivation; assert `current_version` did NOT advance and the rule remains review-eligible on its prior snapshot (FR-010/SC-006)
- [ ] T019 [P] [US2] Add `#[test] #[serial]` unchanged-content no-op test in `src-tauri/src/storage.rs`: re-derive with identical content; assert no version bump and no eligibility change (FR-011)
- [ ] T020 [US2] Run the regression guards in `src-tauri/src/storage.rs`: `store_learned_rule_on_conflict_is_suppression_sticky` and `eligible_for_review_enforces_min_cluster_uniformly_across_streams` pass unchanged

### lat.md sync (US2)

- [ ] T021 [US2] Update `lat.md/features.md` "Learning System → Review Eligibility Gate" (~features.md:101-104): `current_version` advances AFTER the new version's `rule_evidence_citations` are persisted; state the invariant "current_version always resolves to a version with its evidence citations"
- [ ] T022 [US2] Add dated reconciliation lines to `specs/005-learning-system-hardening/data-model.md` "As-built reconciliation (a)" and `specs/005-learning-system-hardening/contracts/rule-governance.md` re-derivation note (feature 006 moves the bump to post-citation-persist in `write_rule_files`; `current_version` stays the marker; still no `pending_changed` column); run `lat check`

**Checkpoint US2**: Per-track verify — `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`; `lat check` all green for the US2 changes in isolation.

---

## Phase 5: Polish & Cross-Cutting (Integrated)

**Purpose**: Authoritative post-join verification and delivery.

- [ ] T023 Integrate Track A (US1) + Track B (US2) onto branch `006-learning-hardening-followups`
- [ ] T024 Run the AUTHORITATIVE integrated 0-warning baseline from repo root: `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`; `npm run build`; `lat check` — all must pass with zero new warnings (this gate, not any per-track result, is authoritative)
- [ ] T025 Execute the `quickstart.md` manual V-acceptance: confinement disclosure on a host with vs. without bwrap (US1); pending-rule review-queue stability across re-derivation (US2)
- [ ] T026 Create one squashed conventional commit on branch `006-learning-hardening-followups` (single bare `git commit`, literal `-m`, ≤72-char subject, wrapped body, no AI-attribution lines) — only after T024/T025 pass

---

## Dependencies & Execution Order

- **Setup (T001–T002)** → blocks everything.
- **Foundational (T003–T004)** → confirms test substrate; T003 ‖ T004.
- **US1 (T005–T013)** and **US2 (T014–T022)** are independent and run as two parallel tracks (disjoint files).
  - Within US1: T005 → {T006, T007} → T008 → T009; tests T010 ‖ (after T005), T011 (after T005–T007), T012 (after T009); T013 after impl.
  - Within US2: T014 → T015 → T016; tests T017 ‖ T018 ‖ T019 (after T016), T020 after T016; T021/T022 after T016.
- **Polish (T023–T026)** → strictly after both tracks; T024 is the authoritative gate; T026 last and only on the green gate.

## Parallel Execution Strategy

Two subagents, one per track, in isolated worktrees:

- **Track A subagent**: T005→T013 (`src-tauri/src/cc_client.rs`, `src/types.ts`, `src/components/learning/RunHistory.tsx`, `lat.md/backend.md`).
- **Track B subagent**: T014→T022 (`src-tauri/src/storage.rs`, `src-tauri/src/learning.rs`, `lat.md/features.md`, `specs/005-*` reconciliation lines).

Disjoint file sets → no merge conflicts expected. The integrated post-join baseline (T024) is authoritative per project convention; per-track checkpoints are advisory.

## Implementation Strategy

- **MVP** = US1 alone (P1, security/privacy honesty) — independently shippable.
- **Increment 2** = US2 (P2, correctness) — independently shippable.
- Either order works; parallel is preferred. No production code lands until plan approval (granted) and each track passes its checkpoint; nothing ships until the integrated T024 gate is green.
