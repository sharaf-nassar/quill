# Implementation Plan: Learning System Hardening Follow-ups

**Branch**: `006-learning-hardening-followups` | **Date**: 2026-05-18 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `specs/006-learning-hardening-followups/spec.md`

## Summary

Close the two deferred Medium defects from the feature-005 code review. **Follow-up A**: the Linux `bwrap`-absent inference path applies only process namespaces (no filesystem confinement) yet records a label that implies real FS isolation — fix by making the recorded confinement vocabulary honest and surfacing the reduced state to the operator (no hand-rolled FS sandbox). **Follow-up B**: re-deriving a pending `awaiting_review` rule advances `current_version` before the new version's evidence citations are written — fix by advancing `current_version` only after the new-version citations are persisted, so the "current version always has its evidence" invariant holds by construction with no migration. Both are independent, small, and verified on the FR-021 CI-gated learning surface with the existing deterministic harness.

## Technical Context

**Language/Version**: Rust (workspace edition, `src-tauri/`); TypeScript + React (Vite, `src/`)
**Primary Dependencies**: Tauri, rusqlite (SQLite), serde/serde_json, tokio, serial_test, tempfile; `lat.md` CLI for the knowledge graph. **No new dependency or crate introduced by the recommended options.**
**Storage**: SQLite `usage.db`, schema at migration 25. Recommended Follow-up B option introduces **no migration** (honors FR-013 / feature-005 data-model constraint (a)).
**Testing**: `cargo test --lib` (`#[test]`/`#[tokio::test]` + `#[serial]`, `TempDir`/`init_storage_in`, the `cc_client` `#[cfg(test)]` `InferenceDoubleGuard` offline scripted-inference double); frontend `npm run build` (tsc) + a `RunHistory` render assertion. CI release gate = feature-005 FR-021.
**Target Platform**: Linux development host. macOS/Windows confinement is out of scope and unchanged (the macOS cfg path cannot compile here; feature-005 FIX #3 already scoped macOS reads).
**Project Type**: Desktop app — Rust (Tauri) backend + React frontend.
**Performance Goals**: No perf change. Follow-up B keeps the single indexed point-read eligibility model (no N+1); Follow-up A adds no runtime cost (vocabulary + UI string).
**Constraints**: 0-warning baseline — `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings` (forced clean re-lint; `cargo check` does not run clippy and cached clippy hides warnings); `cargo test --lib`; `npm run build`; `lat check`. Never fail-closed. Immutability/coding-style per repo rules. Strict commit hooks (one bare `git commit`, literal `-m`, ≤72-char conventional subject, wrapped body, no AI-attribution lines).
**Scale/Scope**: Two contained defect fixes on disjoint file sets; no architectural change.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

`.specify/memory/constitution.md` is an **unratified template** (all placeholders). No project-specific principles are defined, so the binding gates are the de-facto repo conventions in `CLAUDE.md` / user global rules:

| Gate | Status | Notes |
|------|--------|-------|
| lat.md kept in sync + `lat check` passes | PASS (planned) | Sync points enumerated per follow-up; `lat check` in the verification baseline. |
| 0-warning verification baseline | PASS (planned) | fmt + forced-clean clippy `-D warnings` + `cargo test --lib` + `npm run build`. |
| FR-021 CI test gate for learning logic | PASS (planned) | New deterministic unit tests on the gated surface; no live process/network. |
| No new crate/dependency without user approval | PASS | Recommended options add none (A2 avoids the unsafe/syscall path; B3 needs none). |
| No DB migration unless justified | PASS | Recommended Follow-up B option (B3) introduces no migration. |
| Never fail-closed | PASS | Follow-up A keeps "degrade and record"; learning always continues. |
| Strict commit-hook compliance | PASS (deferred) | A single squashed commit only on explicit user go-ahead after approval. |

**Result**: No violations. Complexity Tracking left empty.

## Project Structure

### Documentation (this feature)

```text
specs/006-learning-hardening-followups/
├── plan.md              # This file (/speckit-plan output)
├── spec.md              # /speckit-specify output
├── research.md          # Phase 0 — design options + decisions (R-A1..A3, R-B1..B3)
├── data-model.md        # Phase 1 — schema impact (none) + metadata/type deltas + reconciliation
├── quickstart.md        # Phase 1 — maintainer verification walkthrough
├── contracts/
│   └── confinement-and-atomicity.md   # Phase 1 — honest-confinement + version/evidence-atomicity contracts
├── checklists/
│   └── requirements.md  # spec quality checklist (all pass)
└── tasks.md             # Phase 2 — /speckit-tasks (NOT created here; after approval)
```

### Source Code (repository root)

```text
src-tauri/src/
├── cc_client.rs    # Follow-up A: SandboxKind, detect_sandbox_kind, apply_sandbox,
│                   #   InferenceCallMetadata, sandbox metadata test + InferenceDoubleGuard
├── storage.rs      # Follow-up B: store_learned_rule, persist_evidence_citations,
│                   #   eligible_for_review; learned_rules/rule_evidence_citations schema
└── learning.rs     # Follow-up B: write_rule_files ordering; encode_inference_metadata

src/
├── types.ts                              # Follow-up A: RunInferenceCall / RunInferenceSummary
└── components/learning/RunHistory.tsx    # Follow-up A: confinement disclosure surface

lat.md/
├── backend.md      # Follow-up A sync: Claude Code Inference Client confinement + metadata paragraphs
└── features.md     # Follow-up B sync: Learning System → Review Eligibility Gate ordering + invariant
```

**Structure Decision**: Reuse the existing repo layout — no new top-level directories or modules. The two follow-ups touch **disjoint file sets** (A: `cc_client.rs` + `src/`; B: `storage.rs` + `learning.rs`), so they form two independent build tracks that can be implemented in parallel; the integrated post-join verification baseline is authoritative.

---

## Follow-up A — Honest inference confinement + operator disclosure

### A.1 Problem (precise)

- `src-tauri/src/cc_client.rs::detect_sandbox_kind` (cc_client.rs:176-211): Linux probes `bwrap` → `unshare` → `SandboxKind::None`.
- `src-tauri/src/cc_client.rs::apply_sandbox` (cc_client.rs:311-455). The `SandboxKind::Unshare` branch (cc_client.rs:393-421) wraps the command in `unshare --mount --pid --ipc --uts --fork --kill-child` with **no `--ro-bind`, no `--tmpfs /tmp`, no RW carve-out (`rw_dir` is ignored), no `pivot_root`**. `unshare --mount` clones the parent mount table; the child retains full read/write visibility of `$HOME`, `~/.claude` (Anthropic API key), `~/.config`, the SQLite `usage.db`, and project trees. The bwrap branch (cc_client.rs:332-391) by contrast is deny-by-default with RO system + resolved claude/node binds and a single RW bind of exactly the per-call temp dir.
- `src-tauri/src/cc_client.rs::SandboxKind` (cc_client.rs:118-150): the enum doc groups `Unshare` with `Bwrap` as real "FS/IPC/PID namespace" confinement; `as_str()` → `"unshare"`.
- `InferenceCallMetadata.sandbox` (cc_client.rs:507-539) emits `"unshare"` on success (`metadata_from_envelope` cc_client.rs:706-752), failure (`failed_metadata` cc_client.rs:644-657), and the offline double (`doubled_metadata`); persisted to `learning_runs.inference_metadata` JSON via `learning.rs::encode_inference_metadata` (learning.rs:31-45).
- Frontend `src/types.ts` `RunInferenceCall`/`RunInferenceSummary` (types.ts:420-453) have no sandbox field; `src/components/learning/RunHistory.tsx` never surfaces confinement. `lat.md/backend.md:355-368` explicitly states it is "storage-only … the UI does not yet surface it".
- **Conformance gap**: feature-005 `FR-005`/`SC-013` (spec.md:260-267, 430-434) require the confined process "cannot read or modify data outside a disposable, isolated workspace"; research R-7 (research.md:263-310) accepted "degrade and record", but the recorded `unshare` label is documented as real FS confinement → it **overstates** the protection. The reported defect is the *misrepresentation + non-disclosure*, not the degradation itself (degradation + never-fail-closed is an accepted R-7 decision).

### A.2 Design options

| Option | Approach | Pros | Cons |
|--------|----------|------|------|
| **A1** Hand-rolled user+mount ns | Replace the `unshare(1)` wrapper with an in-process pre-exec creating `CLONE_NEWUSER\|NEWNS`, uid/gid map + setgroups-deny, `MS_REC\|MS_PRIVATE`, tmpfs root, RO system + claude/node binds, RW = per-call temp dir, `pivot_root`, network preserved, never fail-closed. | Actually closes FR-005 on bwrap-absent Linux; `sandbox:"unshare"` becomes truthful; matches bwrap. | Exactly what R-7 "Alternatives" **rejected** (hand-rolled ns "heavy/error-prone"); large unsafe/syscall surface (uid_map/setgroups ordering, mount propagation, pivot_root, CLOEXEC, fork/exec under tokio); needs `nix`/`libc` or a new crate (user approval; verify-before-implementing); heavy test+manual burden; a subtle hole gives **false confidence** — worse than honest disclosure when bwrap is a one-line install. |
| **A2** Honest vocabulary + disclosure (**recommended**) | Make `SandboxKind` express "process/namespace isolation only, NO filesystem confinement"; record the bwrap-absent Linux path under that honest tag (keep the PID/IPC unshare wrapper as non-FS defense-in-depth); add a `confinement` field to the run-inference frontend types + summary projection; surface a distinct marker + remediation hint ("No filesystem confinement on this host — install bwrap for full isolation") in `RunHistory.tsx`. Update SC-013 test, feature-005 data-model/research reconciliation, `lat.md`. | Fixes the actual reported defect (misrepresentation + non-disclosure) at low risk; no unsafe/syscall/crate; consistent with R-7 "degrade and record"; fully deterministic-testable; actionable for the operator; SC-013 invariant preserved (still 100% recorded, now truthfully). | Does not close FS exposure on bwrap-absent hosts — bwrap becomes effectively required for the FR-005 guarantee on Linux (acceptable: documented, remediable, never fail-closed); vocabulary change touches the SC-013 test + feature-005 docs (additive, manageable). |
| **A3** Metadata/docs only | A2's vocabulary correction but no UI surface. | Smallest change; corrects the persisted record + docs. | Operator cannot *see* the reduced confinement when reading run history (the brief explicitly asks for the RunHistory UI surface); under-delivers. |

### A.3 Recommendation — **A2**

Fully addresses the reported defect at low risk, honors the feature-005 never-fail-closed / "degrade and record" decision, and matches the brief's explicit ask to surface in `InferenceCallMetadata` + RunHistory UI + docs. A1 is rejected as primary (R-7 already rejected hand-rolled namespaces; false-confidence risk exceeds benefit when bwrap is vetted + trivially installable) — record it as a possible *future, opt-in, config-gated, never-default, never-fail-closed* enhancement. A3 is rejected as under-delivering.

**Sub-decision (resolved here, not a spec ambiguity)**: represent the bwrap-absent Linux path as a **new explicit `SandboxKind` variant** (e.g. `SandboxKind::ProcessOnly`, `as_str()` → a distinct stable tag) carrying an `is_fs_confined()` = `false` classification, rather than collapsing to `None`. Rationale: the closed-vocabulary audit signal that a *process* namespace wrapper actually ran is strictly more information than `None`; the SC-013 test already enumerates a per-platform set that is trivial to extend. `Bwrap`/`SandboxExec` ⇒ `is_fs_confined()` = `true`; `ProcessOnly`/`JobObject`/`None` ⇒ `false`.

### A.4 Test strategy (FR-021, deterministic, offline)

- **Adjust** `sandbox_metadata_is_recorded_for_every_call` (cc_client.rs:1660-1737, `#[tokio::test] #[serial]`, uses `set_inference_double_scoped` → `InferenceDoubleGuard`): extend the closed set + Linux `platform_expected` with the new tag; assert `as_str()` round-trips it; keep host-agnostic assertions (membership/classification, never the host's actual mechanism).
- **New pure test** (no spawn, no `#[serial]`): for every `SandboxKind`, assert `as_str()` is in the closed set and `is_fs_confined()` matches the FS/non-FS classification table.
- **Frontend**: a `RunHistory` render assertion — a non-FS-confined run shows the distinct marker + hint; an FS-confined run does not. Type change verified by `npm run build` (tsc) in the baseline.
- **Discipline**: `detect_sandbox_kind()` is host-dependent; tests assert classification + closed-set membership only (CI may lack bwrap), mirroring the existing SC-013 test.

### A.5 lat.md sync points

- `lat.md/backend.md` "Claude Code Inference Client" (backend.md:355-368): rewrite the sandbox paragraph — the bwrap-absent Linux path is process-namespace isolation only with **no filesystem confinement**, recorded under the honest tag; update the metadata paragraph ("storage-only … the UI does not yet surface it" → now surfaced with a remediation hint). Update the `[[src-tauri/src/cc_client.rs#SandboxKind]]` ref if a variant is added.
- `lat check` must pass. No `lat.md/tests.md` exists, so no `require-code-mention` test-spec obligation for this surface (confirmed); verify again at task time.

---

## Follow-up B — Atomic rule version vs. evidence citations

### B.1 Problem (precise)

- `src-tauri/src/storage.rs::store_learned_rule` (storage.rs:4693-4785): `INSERT … ON CONFLICT(name) DO UPDATE … current_version = CASE WHEN learned_rules.lifecycle='awaiting_review' AND excluded.content IS NOT NULL AND excluded.content IS NOT learned_rules.content THEN current_version+1 ELSE current_version END` (storage.rs:4759-4764). Single auto-committed statement on the shared `Mutex<Connection>` (storage.rs:4694); no enclosing transaction.
- `src-tauri/src/storage.rs::persist_evidence_citations` (storage.rs:5245-5304): reads `(rule_id, current_version)` at entry (storage.rs:5251-5258) **before** its own tx begins (storage.rs:5266); DELETE+INSERT `rule_evidence_citations` at that `rule_version`; commit (storage.rs:5301). Non-fatal in the caller.
- `src-tauri/src/storage.rs::eligible_for_review` (storage.rs:5359-5458): `SELECT COUNT(*) FROM (SELECT DISTINCT kind, ref_id FROM rule_evidence_citations WHERE rule_id=?1 AND rule_version=?2)` with `?2 = current_version` (storage.rs:5419-5426); gate needs `resolved_distinct_refs >= 3` (storage.rs:5431) AND `distinct_sources >= 1` (storage.rs:5437-5455).
- `src-tauri/src/learning.rs::write_rule_files` (learning.rs:1587; ordering learning.rs:1712-1782): `store_learned_rule` (bumps version) → `persist_evidence_citations` (non-blocking: logs + continues on error) → `eligible_for_review`.
- **Root**: the version bump and the new-version citation write are two separate committed units. The invariant "`current_version` references a version that has its citations" is violated between them: **transient** for any concurrent/interleaved eligibility reader, and **persistent** if `persist_evidence_citations` fails (non-blocking in `write_rule_files`) — the rule is then stuck with `current_version` pointing at a citation-less version and silently, permanently leaves the human review queue until a later successful re-derivation. The persistent case is the higher-severity manifestation.
- **Constraint**: data-model.md "As-built reconciliation (a)" — the pending marker IS the `current_version` bump; migration 25 has no `pending_changed` column; keep that unless a new additive migration is justified (FR-013). research R-6 (research.md:218-261) governs the gate ordering ("gate moves to `write_rule_files` after `store_learned_rule`"). Contract `rule-governance.md` says re-derivation "bumps a pending version … never silent overwrite" (its stale "sets `pending_changed=1`" line is already reconciled away by data-model (a)).

### B.2 Design options

| Option | Approach | Pros | Cons |
|--------|----------|------|------|
| **B1** One transaction | New `Storage` method runs the ON CONFLICT upsert (incl. version bump) + the citation DELETE/INSERT (+ optionally the lifecycle transition) in one `conn.transaction()`; `write_rule_files` calls it. | Strict atomicity — no reader and no failure ever sees version-without-citations; no migration; directly fixes the stated root. | Largest blast radius — merges two well-tested public methods + call site; must move `persist_evidence_citations`' pre-tx read inside; **semantic change**: citation failure now rolls back the α/β + content merge too (feature-005 "merge-always" changes) — accept it or add savepoint nuance; must preserve `store_learned_rule`'s suppression-sticky CASE exactly (regression risk vs storage.rs:10048-10148). |
| **B2** Decouple count from current_version | Change `eligible_for_review`'s citation queries (storage.rs:5419-5426, 5437-5455) from `rule_version = current_version` to the greatest cited version `<= current_version`. Bump + citation write stay separate. | Smallest change (one predicate, two spots); no migration; tolerant of both the transient window and a failed/late citation persist (rule stays eligible on its last good snapshot); no merge-path behavior change. | **Semantic drift** — eligibility judged against the previous content version's evidence until the new snapshot lands; defensible (same pending review; retention-proof snapshot) but subtler; weakens the "evidence matches the exact pending text" intuition; must prove it can never make a never-supported rule eligible (it cannot — only preserves already-earned eligibility). |
| **B3** Bump after citations persist (**recommended**) | `store_learned_rule` keeps merging content + α/β and *detecting* a pending content change but no longer bumps `current_version` in the CASE; it surfaces a "pending-changed" signal. `write_rule_files` then: compute `target = current_version + 1`; `persist_evidence_citations` writes the new snapshot at `target`; then atomically `UPDATE … SET current_version = target` (same tx as the citation write, or an immediately-following guarded update). On citation failure → no bump → rule stays at the old version which still has its citations. | Invariant true by construction; closes **both** the transient window and the persistent-failure case (the real damage); **no migration** (keeps current_version-as-marker, honors data-model (a)); preserves feature-005 merge-always (α/β still merges even if citations fail); naturally unit-testable. | Relocates the pending-marker write out of `store_learned_rule`'s CASE into `write_rule_files` (small, but update contract + data-model (a) wording); brief reversed window where content=new but current_version still old (benign — old version still has citations, gate stays satisfied; document it). |

### B.3 Recommendation — **B3**

Makes the safety invariant structural rather than timing-dependent, fixes the persistent-failure case (a human-pending rule silently and permanently leaving the queue — the real damage), requires no migration (honors FR-013 / data-model (a)), and preserves feature-005's merge-always semantics. B1 also closes it but with a larger refactor and a behavior change to citation-failure handling. B2 is cheapest but introduces eligibility-vs-pending-content version drift that is harder to reason about. A `pending_version` column (additive migration 26) is **rejected** to honor data-model (a); B3 needs no schema change.

**Migration impact**: none. **Behavioral delta**: only that `current_version` advances one step later (after citations), which is the fix; merge-always and suppression-sticky are unchanged.

### B.4 Test strategy (FR-021, deterministic; `#[test] #[serial]` + `TempDir` + `init_storage_in`, pattern per `eligible_for_review_enforces_min_cluster_uniformly_across_streams` storage.rs:10658-10800 and `store_learned_rule_on_conflict_is_suppression_sticky` storage.rs:10048-10148)

- **Eligibility preserved across re-derivation**: seed an `awaiting_review` rule at v1 with ≥3 distinct refs + ≥1 source ⇒ eligible. Re-derive with changed content via the (new) combined path. Assert eligibility stays true; `current_version` becomes v2 only after v2 citations exist; at no observable point does `current_version` reference a 0-ref version.
- **Citation-failure injection** (FR-010/SC-006): force the citation step to fail on re-derivation (e.g. resolved evidence yielding zero insertable rows, or a test seam). Assert `current_version` did NOT advance and the rule remains review-eligible on its prior snapshot.
- **Unchanged-content no-op** (FR-011): re-derive with identical content ⇒ no version bump, no eligibility change (guards B3 against regressing the no-op path).
- **Regression**: `store_learned_rule_on_conflict_is_suppression_sticky` and `eligible_for_review_enforces_min_cluster_uniformly_across_streams` pass unchanged.

### B.5 lat.md sync points

- `lat.md/features.md` "Learning System → Review Eligibility Gate" (features.md:101-104): update the ordering sentence — `current_version` advances **after** the new version's `rule_evidence_citations` are persisted; state the invariant "current_version always resolves to a version with its evidence citations".
- Add a **dated reconciliation line** (mirroring how feature 005 wrote its own as-built note — do not rewrite feature-005 history) to `specs/005-learning-system-hardening/data-model.md` "As-built reconciliation (a)" and `specs/005-learning-system-hardening/contracts/rule-governance.md` re-derivation note: feature 006 moves the bump to post-citation-persist in `write_rule_files`; `current_version` remains the pending marker; still no `pending_changed` column.
- `lat check` must pass.

---

## Phase 2 — Dependency-ordered task list (build sequence)

Formalized via `/speckit-tasks` only after plan approval. Tracks A and B are independent (disjoint files) and parallelizable as two subagents; the integrated post-join verification is authoritative.

**Track A (cc_client.rs + src/):**
1. **A-T1** Add `SandboxKind::is_fs_confined()` classifier + the new honest `ProcessOnly` variant; update `as_str()` + enum doc. *(foundation)*
2. **A-T2** Record the bwrap-absent Linux path under the honest tag in `detect_sandbox_kind`/`apply_sandbox`; keep the PID/IPC unshare wrapper as non-FS defense-in-depth. *(after A-T1)*
3. **A-T3** Update `InferenceCallMetadata.sandbox` doc; confirm success/failure/doubled paths emit the honest tag. *(after A-T1)*
4. **A-T4** Adjust `sandbox_metadata_is_recorded_for_every_call`; add the pure `is_fs_confined` mapping test. *(after A-T1..A-T3)*
5. **A-T5** Add `confinement` to `RunInferenceCall`/`RunInferenceSummary` + the backend summary projection that feeds them; surface marker + remediation hint in `RunHistory.tsx`. *(after A-T3)*
6. **A-T6** `RunHistory` render assertion; `npm run build` (tsc) green. *(after A-T5)*
7. **A-T7** lat.md sync (backend.md confinement + metadata paragraphs; SandboxKind ref). *(after A-T1..A-T5)*

**Track B (storage.rs + learning.rs):**
8. **B-T1** Stop bumping `current_version` in `store_learned_rule`'s ON CONFLICT CASE; surface a "pending-changed" signal; preserve the suppression-sticky CASE exactly. *(foundation)*
9. **B-T2** Persist new-version citations at `target = current_version+1`, then atomically bump `current_version` to `target` (same tx as the citation write or an immediately-following guarded update); rewire `write_rule_files` ordering. *(after B-T1)*
10. **B-T3** Confirm `eligible_for_review` now always reads a cited version (no query change under B3); add invariant assertions. *(after B-T2)*
11. **B-T4** Tests: eligibility-preserved; citation-failure injection; unchanged-content no-op; regression suppression-sticky + min-cluster. *(after B-T2)*
12. **B-T5** lat.md sync (features.md ordering + invariant) + dated reconciliation lines into feature-005 data-model.md (a) / contracts rule-governance.md. *(after B-T2)*

**Integration / cross-cutting:**
13. **X-T1** Authoritative integrated baseline on the joined branch: `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`; `npm run build`; `lat check`. *(after all A-/B- tasks)*
14. **X-T2** Quickstart manual V-acceptance (host with/without bwrap → disclosure check; re-derivation queue-stability check). *(after X-T1)*
15. **X-T3** One squashed conventional commit per the strict hook rules — **only on explicit user go-ahead after approval**. *(after X-T1/X-T2)*

## Complexity Tracking

No constitution violations; section intentionally empty.

## As-built reconciliation (2026-05-18 — integration gate)

Two refinements surfaced at the authoritative integrated `clippy -D warnings`
gate and were resolved in favor of single-source-of-truth (this is a
hardening feature):

- **Follow-up A classifier**: implemented as one `pub(crate)` tag-keyed free
  fn `cc_client::sandbox_tag_is_fs_confined(&str)` (the recorded value is
  always a transported/persisted string), not a duplicate
  `SandboxKind::is_fs_confined()` method. The decode/projection path and the
  totality test both use it; the test drives it via `as_str()` over every
  variant, so the closed-set + classification totality guarantee is
  unchanged. Contract C-A2 / data-model / quickstart updated to match.
- **Follow-up B**: the orphaned `persist_evidence_citations` (its only
  production caller, `write_rule_files`, now uses
  `persist_citations_and_advance_version`) was removed rather than left
  behind `#[allow(dead_code)]`. Its 7 feature-005 regression-test call
  sites were migrated to `persist_citations_and_advance_version(.., false)`,
  whose `pending_changed == false` path is byte-for-byte the old behavior
  (verified by reading both bodies and by `cargo test --lib`). `lat.md`
  links repointed to the surviving sole writer.
