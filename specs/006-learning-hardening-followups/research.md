# Phase 0 Research — Learning System Hardening Follow-ups

Design-decision record for the two deferred feature-005 defects. No `NEEDS CLARIFICATION` remained from the spec (the brief was exhaustive and delegated option selection to this phase).

## R-A — Inference confinement honesty (Follow-up A)

**Decision**: Adopt **A2** — make the recorded confinement vocabulary honest and surface the reduced state to the operator; do **not** hand-roll a filesystem namespace. Add `SandboxKind::is_fs_confined()` and a new explicit `ProcessOnly` variant for the Linux `bwrap`-absent path (the PID/IPC `unshare` wrapper is kept as non-FS defense-in-depth but no longer recorded as FS confinement). Surface a distinct marker + remediation hint in `RunHistory.tsx` via a new `confinement` field on the run-inference frontend types/summary.

**Rationale**: The reported defect is *misrepresentation + non-disclosure*, not the degradation itself — feature-005 R-7 explicitly accepted graceful degradation and never-fail-closed. A2 corrects exactly the reported gap at low risk, adds no unsafe/syscall code and no new crate, keeps the SC-013 "100% recorded" invariant (now truthfully), and gives the operator an actionable next step (`install bwrap`). A new explicit variant is preferred over collapsing to `None` because "a process namespace wrapper actually ran" is strictly more audit information than "nothing".

**Alternatives considered**:
- **A1 hand-rolled user+mount namespace + pivot_root** — rejected as primary. R-7 "Alternatives" already rejected hand-rolled namespaces ("heavy/error-prone"); large unsafe surface (uid_map/setgroups ordering, mount propagation, pivot_root, CLOEXEC, tokio fork/exec), a new crate, and a subtle hole would give *false confidence* — strictly worse than honest disclosure when `bwrap` is a vetted one-line install. Logged as a possible future opt-in, config-gated, never-default, never-fail-closed enhancement.
- **A3 metadata/docs only** — rejected: no operator-visible disclosure; the brief explicitly requires the RunHistory UI surface.

**2026-05-19 (feature 007 reconciliation)**: The `SandboxKind::ProcessOnly` variant introduced by R-A is **retired** in feature 007. Reason: its `unshare(2)` wrapper required the same `CLONE_NEWUSER` capability AppArmor blocks on Ubuntu 24.04+, so the tier was theatrical on the same hosts that broke bwrap — no FS-confinement value either way. The honest-tag-vocabulary contribution from R-A — the `sandbox_tag_is_fs_confined` classifier and feature 006-A's RunHistory disclosure UI (marker + remediation hint) — is **kept and built upon** by feature 007: the classifier just gains `"landlock"` → true alongside the existing entries; the UI is unchanged. The closed write vocabulary on Linux contracts from `{"bwrap","process-only","sandbox-exec","job-object","none"}` to `{"landlock","bwrap","sandbox-exec","job-object","none"}`; the decode classifier stays tolerant of retired tags forever (feature 007 contract C-D). See `specs/007-landlock-inference-sandbox/`.

## R-B — Rule version vs. evidence-citation atomicity (Follow-up B)

**Decision**: Adopt **B3** — `store_learned_rule` stops bumping `current_version` in its `ON CONFLICT` CASE and instead surfaces a "pending-changed" signal; `write_rule_files` persists the new-version `rule_evidence_citations` at `target = current_version + 1` and only then atomically bumps `current_version` to `target` (same transaction as the citation write, or an immediately-following guarded `UPDATE`). On citation failure the bump does not happen, so the rule stays at a version that still has its evidence.

**Rationale**: Makes the invariant "`current_version` always resolves to a version that has its evidence citations" true *by construction* rather than by timing. It closes both the transient (concurrent-reader) window and the higher-severity persistent case (a human-pending rule silently and permanently leaving the review queue after a non-blocking citation-write failure). It needs **no migration** — `current_version` remains the pending-change marker, honoring FR-013 and feature-005 data-model "As-built reconciliation (a)" — and preserves feature-005's "α/β + content merge always happens" semantics (only the version pointer moves later).

**Alternatives considered**:
- **B1 single transaction over upsert + citation write** — rejected as primary: largest blast radius (merges two well-tested public methods + their call site), and folding the citation write into the upsert tx changes feature-005 merge-always semantics (a citation failure would also roll back the α/β/content merge). Correct, but heavier and behavior-changing for marginal benefit over B3.
- **B2 count eligibility at the greatest cited version ≤ current_version** — rejected: cheapest, but introduces eligibility-vs-pending-content version drift that is subtler to reason about and slightly weakens the "evidence matches the exact pending text" intuition.
- **New `pending_version` column (additive migration 26)** — rejected to honor data-model (a) (pending marker is the version bump; no dedicated column). B3 needs no schema change, so the migration is unjustified.

## Cross-cutting decisions

- **Verification surface**: both follow-ups land on the FR-021 CI-gated learning surface; all new behavior covered by deterministic unit tests using the existing `TempDir`/`#[serial]`/`init_storage_in` harness and the `cc_client` `#[cfg(test)]` `InferenceDoubleGuard` offline double. No test requires a live external process or network.
- **Parallelization**: A (`cc_client.rs` + `src/`) and B (`storage.rs` + `learning.rs`) touch disjoint files → two parallel implementation tracks; the integrated post-join baseline (`fmt` + forced-clean `clippy -D warnings` + `cargo test --lib` + `npm run build` + `lat check`) is authoritative.
- **No fail-closed, no new crate, no migration** for the recommended options — preserves every feature-005 invariant.
