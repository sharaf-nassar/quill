---
description: "Task list for Learning System Hardening implementation"
---

# Tasks: Learning System Hardening

**Input**: Design documents from `/specs/005-learning-system-hardening/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Test tasks ARE included — FR-021 + spec Assumptions explicitly
require automated learning-logic tests + a CI gate (overrides the project's
"tests only on request" default for this surface). Test scope is the
learning-logic surface, not blanket UI TDD.

**Organization**: By user story (US1–US5 from spec.md), in priority order.
Cross-cutting integration constraints from `research.md` are encoded in the
Foundational phase and the Dependencies section.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: parallelizable (different files, no incomplete-task dependency)
- **[USx]**: owning user story (story phases only)
- Backend `src-tauri/src/`, frontend `src/`, fixtures `src-tauri/tests/`,
  CI `.github/workflows/`, docs `lat.md/`

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Module/CI/fixture scaffolding; baselines.

- [x] T001 [P] Create `src-tauri/src/redaction.rs` (stub `pub fn redact(&str)->String`) and add `mod redaction;` to `src-tauri/src/lib.rs`
- [x] T002 [P] Create `src-tauri/src/eval_harness.rs` (stub) and add `mod eval_harness;` to `src-tauri/src/lib.rs`
- [x] T003 [P] Create `src-tauri/tests/fixtures/replay_set/` with `manifest.json` skeleton (`replay_set_version`, `baseline_assistant_model`, `frozen_at`, `schema_version`, `cases:[]`)
- [x] T004 [P] Add `.github/workflows/ci.yml` skeleton (`pull_request` + `push:main`; fmt/clippy/test job placeholders)
- [x] T005 Record build baseline: `cargo build --manifest-path src-tauri/Cargo.toml` green and current `cargo clippy` warning count noted in the PR description
- [x] T006 [P] Capture the **SC-011 pre-remediation learning-value baseline** to `specs/005-learning-system-hardening/baseline.md` (active + discovered rule counts per provider scope, and a maintainer reviewer-usefulness sample of ≥10 rules) on `main` **before any pipeline change** — this is the only SC-011 comparison anchor and is unrecoverable once changes land

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared schema + primitives that multiple user stories depend on.

**⚠️ CRITICAL**: Encodes research.md constraints C1 (migration 25 shared,
landed first) and C3 (redaction core before citation snapshots). No US2/US3/US4
work begins until this phase completes.

- [x] T007 Implement **migration 25** in `src-tauri/src/storage.rs` (single transactional, idempotent block; `table_has_column` guards; `CREATE TABLE/INDEX IF NOT EXISTS`; `INSERT INTO schema_version VALUES (25)`): `learned_rules` += `lifecycle,origin_run_id,origin_model,origin_at,current_version,superseded_by`; new `rule_versions`, `rule_evidence_citations`, `rule_tombstones`, `operator_feedback`, `evaluation_results`, `reviewer_overrides`; repurpose `confirmed_projects` — per `data-model.md`
- [x] T008 Add migration-25 regression test in `src-tauri/src/storage.rs` test module (assert `MAX(version)=25`; idempotent re-run; all columns/tables present) — mirror existing `migration_20_*` test
- [x] T009 [P] Implement `redaction::redact` in `src-tauri/src/redaction.rs` per `contracts/redaction.md` (anchored creds + URL userinfo + Shannon-entropy ≥4.0 gated + email-localpart; idempotent `‹redacted›`) + unit tests (idempotence, entropy false-positive guards)
- [x] T010 Extract pure `evidence_weighted_score(alpha,beta,last_evidence_at)->(score,state)` in `src-tauri/src/storage.rs`; route both `get_learned_rules` read sites (`:4338-4342`,`:4382-4385`) through it (no behavior change) + unit test
- [x] T011 [P] Add `#[cfg(test)]`-injectable inference double seam in `src-tauri/src/cc_client.rs` (scripted `StreamFindings`/`EvalVerdict`/`InferenceError`; production free-fn signatures unchanged) per `contracts/evaluation-harness.md`

**Checkpoint**: Schema + redaction core + shared scorer + test seam ready.

---

## Phase 3: User Story 1 - Sensitive data protected end to end (Priority: P1) 🎯 MVP

**Goal**: No secret/PII stored unprotected or sent to any analysis path; the
spawned `claude` CLI is OS-confined where the platform supports it, with
recorded confinement state (C-1, H-3, H-5).

**Independent Test**: Quickstart V1+V7 — seeded secret/PII corpus → 0
unredacted at rest or in any inference input; rules still produced; confinement
state recorded.

- [x] T012 [P] [US1] Redaction adoption tests in `src-tauri/src/redaction.rs` / `prompt_utils.rs` test module: seeded secret/PII corpus → 0 unredacted at rest + per inference input; structure preserved (SC-001)
- [x] T013 [US1] Redact at capture in `src-tauri/src/server.rs` `post_observation` (`tool_input/tool_output/cwd`) before `store_observation_in_background`; `202` stays synchronous after (FR-001)
- [x] T014 [US1] Defense-in-depth redact in `src-tauri/src/storage.rs` `store_observation` before INSERT (no plaintext at rest)
- [x] T015 [P] [US1] Stream B: redact in `src-tauri/src/git_analysis.rs` before `compress_git_data` AND before `git_snapshots.raw_data` cache write (FR-002)
- [x] T016 [P] [US1] Synthesis: redact `memory_context`/`instruction_context` in `src-tauri/src/learning.rs:997-1028` before sanitize/truncate (FR-002)
- [x] T017 [P] [US1] `memory_optimizer::build_prompt` in `src-tauri/src/memory_optimizer.rs`: redact each content field before escape/truncate (FR-002)
- [x] T018 [US1] Invert Stream C order at `src-tauri/src/learning.rs:142` to `compress(redact(raw))` (FR-003)
- [x] T019 [US1] H-3: route `reconcile_learned_rules` 3a/3c + `promote_learned_rule` content through `redact`→`sanitize_rule_content` in `src-tauri/src/storage.rs`; hash raw bytes for change detection (FR-004)
- [x] T020 [US1] Fix `sanitize_rule_content` in `src-tauri/src/learning.rs:1567-1581` to strip code fences per its doc-comment; correct doc to injection-only
- [x] T021 [US1] One-time idempotent redaction backfill of existing `observations` + `git_snapshots.raw_data` (settings sentinel) in `src-tauri/src/storage.rs` (FR-001 backfill assumption)
- [x] T022 [US1] OS sandbox wrapper in `src-tauri/src/cc_client.rs` `build_command`/`invoke_raw` (Linux bwrap→unshare, macOS sandbox-exec, Windows Job Object best-effort; RW = per-call temp dir; network preserved; **never fail closed**); record `sandbox` confinement state on `InferenceCallMetadata` (H-5/FR-005/SC-013)
- [ ] T023 [US1] Validate quickstart V1 + V7 manually

**Checkpoint**: US1 independently testable — privacy + recorded confinement. MVP.

---

## Phase 4: User Story 2 - No rule goes global without review, reversible (Priority: P1)

**Goal**: Human approval is the only path to a global rule; every rule has
provenance, version history, working rollback; deleted rules stay gone
(C-2, C-5, H-4, FR-007/012/013).

**Independent Test**: Quickstart V2+V3 — high-confidence extraction creates no
`.md` without approval; rollback restores prior content; deleted rule does not
resurrect over ≥5 cycles.

- [x] T024 [P] [US2] Governance unit tests in `src-tauri/src/storage.rs`/`learning.rs` test module: tombstone survives re-extraction (C-5), suppression-sticky `ON CONFLICT`, `lifecycle` round-trips `get_learned_rules` un-clobbered, rollback restores version, **and a periodic analysis run with the autonomous branch removed writes 0 `.md` files** (SC-002/SC-004, N7)
- [x] T025 [US2] Delete the `above_threshold` auto-write branch in `src-tauri/src/learning.rs:1497,1530-1549`; extraction writes only `candidate` (FR-007, Q1=A)
- [x] T026 [US2] Implement `lifecycle` state-machine writes in `src-tauri/src/storage.rs`/`learning.rs` per `data-model.md` (distinct from derived `state`)
- [x] T027 [US2] `tombstone_blocks(name)` helper consulted at all 5 name-addressed paths (`store_learned_rule`, `write_rule_files`, `promote_learned_rule`, `reconcile` 3a/3c) + suppression-sticky `ON CONFLICT` in `src-tauri/src/storage.rs` (C-5)
- [x] T028 [US2] `delete_learned_rule` + reconcile step 3b write `rule_tombstones`; reconcile 3a/3c skip tombstoned/rejected names in `src-tauri/src/storage.rs` (FR-010)
- [x] T029 [US2] Harden `promote_learned_rule` into the sole `.md` writer in `src-tauri/src/storage.rs` (precondition `lifecycle='awaiting_review'`; write redacted+sanitized; append `rule_versions` change_kind=promote; populate provenance + `rule_evidence_citations` snapshot) (FR-007/008)
- [x] T030 [US2] Re-derivation idempotency in `store_learned_rule` `ON CONFLICT(name)` for `awaiting_review` rows (UPSERT content, bump pending version, `pending_changed`; never duplicate/auto-approve) in `src-tauri/src/storage.rs`
- [x] T031 [US2] `rollback_rule(name,target_version)` in `src-tauri/src/storage.rs` (append rollback version, restore content/hash/current_version, rewrite `.md`, hash-touch) (FR-009)
- [x] T032 [US2] Legacy archive-then-wipe one-time step in the migration-25 chain in `src-tauri/src/storage.rs` (copy on-disk learned `.md` → `<data_local>/legacy-rules-archive/<ts>/` `0444` + `ARCHIVE_MANIFEST.json`; delete; tombstone DB rows; idempotent sentinel) (FR-012/Q3=C)
- [x] T033 [US2] Run-status tri-state at `update_learning_run` sites in `src-tauri/src/learning.rs` (`running|completed|degraded|failed`; degraded/failed write nothing) (FR-013)
- [x] T034 [US2] IPC auth in `src-tauri/src/lib.rs` (ephemeral capability token + `learning` window-label assertion on promote/delete/approve/reject/rollback/reactivate/suppress/feedback; reads open); clamp HTTP `post_learned_rule` to `candidate` in `src-tauri/src/server.rs` (H-4/FR-011)
- [x] T035 [US2] `reactivate_rule` authorized IPC in `src-tauri/src/lib.rs`/`storage.rs` (only path clearing a tombstone) (FR-010)
- [ ] T036 [US2] Validate quickstart V2 + V3 manually

**Checkpoint**: US1+US2 independently functional — privacy floor + governance.

---

## Phase 5: User Story 3 - Promoted on real evidence, not self-rating (Priority: P2)

**Goal**: Eligibility uses the evidence-weighted score with grounded citations
+ min cluster; verdicts/conflicts act; operator feedback is the primary
outcome signal (C-3, H-1, H-2, M-3, M-4, FR-014..018, FR-029).

**Independent Test**: Quickstart V4 — single-source/low-evidence candidate not
surfaced; unresolvable-citation rule rejected; conflicting pair reconciled;
operator feedback shifts eligibility.

- [x] T037 [P] [US3] Grounding/cluster/verdict unit tests in `src-tauri/src/{learning,storage}.rs` test module: unresolved citation→reject, min-cluster gate, `observation_count` fix, IRRELEVANT decays state, `compute_state` β-override, deterministic supersede (SC-005/SC-006)
- [x] T038 [US3] Add `EvidenceRef{kind,id}` to `StreamPattern`/`AnalysisRule` in `src-tauri/src/models.rs`; thread through `StreamFindings::to_analysis_output`/synthesis (schemars auto-propagates schema) (H-1)
- [x] T039 [P] [US3] Stream A: inject observation `id` into the obs-summary prompt in `src-tauri/src/learning.rs:382-386` (H-1)
- [x] T040 [P] [US3] Stream B: add `%h` short-hash to commit format in `src-tauri/src/git_analysis.rs` (fallback: snapshot HEAD key) (H-1)
- [x] T041 [US3] `Storage::resolve_evidence_refs` + reject zero-resolvable candidates in `write_rule_files` before persist (log+continue) in `src-tauri/src/learning.rs`/`storage.rs` (FR-015)
- [x] T042 [US3] Move evidence-weighted eligibility gate into `write_rule_files` AFTER `store_learned_rule` (`eligible_for_review` via T010 scorer; single point-read, no N+1); set `awaiting_review` vs `candidate`; replace `learning.min_confidence`→`learning.min_eligibility` (default 0.6) in `src-tauri/src/learning.rs`/`storage.rs` (C-3/FR-014; coordinates with T025 — same `write_rule_files` rework)
- [x] T043 [US3] Min evidence cluster in `eligible_for_review` (`resolved_distinct_refs≥3 AND distinct_sources≥1`, uniform A/B/C); thread per-rule resolved citation count into `store_learned_rule` (fix `observation_count=0` for B/C) in `src-tauri/src/storage.rs`/`learning.rs` (H-2/FR-016)
- [x] T044 [US3] Verdicts in `src-tauri/src/learning.rs:1333-1351`: `irrelevant`→`decay_rule_freshness` (one 90d half-life backward, clamped), unknown→logged not dropped; revise `compute_state` to use alpha/beta (`beta>=alpha && beta>=5.0`→`invalidated`) in `src-tauri/src/storage.rs` (M-4/FR-017)
- [x] T045 [US3] Replace advisory consolidation hint with deterministic flag-and-supersede + `record_rule_reconciliation` in `src-tauri/src/learning.rs`/`storage.rs`; repurpose `confirmed_projects` as cross-project distinct-sources (M-3/FR-018)
- [x] T046 [US3] `submit_rule_feedback(name,feedback,note?)` IPC in `src-tauri/src/lib.rs` (validate `is_safe_rule_name`; authorized for `bad`; emit `learning-updated`); upsert `operator_feedback` in `src-tauri/src/storage.rs` (FR-029)
- [x] T047 [US3] Evidence-weighting integration in `src-tauri/src/storage.rs`/`learning.rs`: accept→large α, reject→large β (no tombstone), bad→largest β + tombstone; `W_op` dominates LLM verdicts/self-rating (C-3/FR-029)
- [x] T048 [P] [US3] Operator-feedback UI: extend `src/components/learning/RuleCard.tsx` (3 actions accept/reject/bad; two-step confirm for `bad`); add `submitRuleFeedback` to `src/hooks/useLearningData.ts`; thread via `src/components/learning/LearningWindow.tsx`; `src/types.ts`
- [ ] T049 [US3] Validate quickstart V4 manually

**Checkpoint**: US1+US2+US3 functional — evidence-grounded, human-fed signal.

---

## Phase 6: User Story 4 - Prove a rule helps before it ships (Priority: P2)

**Goal**: Frozen-replay with/without evaluation + regression block + the
FR-021 learning-logic test suite gating CI/release (C-4, FR-019..023).

**Independent Test**: Quickstart V5 — harness returns with/without verdict +
regression signal; regressing rule blocked unless audited override; a known
defect in scoring/state/synthesis fails CI.

- [x] T050 [P] [US4] Author ≥12 pre-redacted replay-set cases across archetypes in `src-tauri/tests/fixtures/replay_set/` + finalize `manifest.json` (pinned baseline model, `frozen_at`) (FR-019)
- [x] T051 [US4] Implement `src-tauri/src/eval_harness.rs`: replay loader, WITH/WITHOUT paired `cc_client` calls (pinned `Sonnet46`, N=3 majority), judge `EvalVerdict` typed, regression dead-band + negative-transfer, calibration κ vs frozen labels, staleness verdict (FR-019/020/023)
- [x] T052 [US4] Persist `evaluation_results` linked to `(rule_name,learning_run_id,replay_set_version)` + `per_case_json` in `src-tauri/src/storage.rs` (FR-022)
- [x] T053 [US4] Promotion coupling in `src-tauri/src/storage.rs`/`lib.rs`: `latest_eval_verdict`/`has_reviewer_override`; approval blocks `regression=1` unless `reviewer_overrides` row; `record_reviewer_override` authorized IPC; uncalibrated/stale→warn (FR-020)
- [x] T054 [P] [US4] Learning-logic unit tests in `src-tauri/src/{storage,learning}.rs` test modules (`wilson_lower_bound`, `compute_state` incl β-override, `freshness_factor`, `eligible_for_review`, synthesis-decision matrix incl insights-only-succeeds, suppression durability) using the T011 double + TempDir/`#[serial]` (FR-021/SC-008)
- [x] T055 [P] [US4] Eval-harness pure-logic unit tests in `src-tauri/src/eval_harness.rs` (dead-band, majority-of-N, κ agreement, staleness) (FR-021)
- [x] T056 [US4] Finalize `.github/workflows/ci.yml` (`cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test`) + wire as `workflow_call` precondition of `.github/workflows/release.yml` (FR-021/SC-008)
- [ ] T057 [US4] Validate quickstart V5 manually (incl. inject known defect → CI red → revert → green)

**Checkpoint**: Loop is measurable and regression-gated.

---

## Phase 7: User Story 5 - See and trust the loop's cost and health (Priority: P3)

**Goal**: Surface per-run cost/latency/model/status; pin synthesis model; fix
retention race; consume summaries; disclose provider asymmetry; doc/test drift
(H-6/7, M-1/2/6, L-1/2/3, FR-024..028).

**Independent Test**: Quickstart V6 — RunHistory shows accurate cost/model/
status incl. `degraded`; unanalyzed observations not purged; shared-rule UI
discloses Codex Bash-only limitation.

- [x] T058 [US5] Decode `learning_runs.inference_metadata`→`RunInferenceSummary` rollup; add field to `LearningRun` in `src-tauri/src/models.rs`; extend `get_learning_runs` SELECT + tolerant decode in `src-tauri/src/storage.rs` (H-6/FR-024)
- [x] T059 [P] [US5] RunHistory UI: cost/model/inference-time rows + `degraded` icon (amber) + derived consecutive-failure banner in `src/components/learning/RunHistory.tsx`; `src/types.ts` (H-6/L-3)
- [x] T060 [P] [US5] Pin synthesis model to `Model::Sonnet46` at `src-tauri/src/learning.rs:826`; update synthesis log strings (H-7/FR-025)
- [x] T061 [US5] Retention watermark + atomic cleanup in `src-tauri/src/storage.rs` `cleanup_old_observations` (cutoff never newer than last completed/degraded run start; zero-success→delete nothing; summarize+delete in one transaction) (M-2/FR-026)
- [x] T062 [US5] Consume `observation_summaries` (`get_observation_summaries` accessor → analytics trend tail) + tighten the `LIKE '%error%'` tally at write in `src-tauri/src/storage.rs` (M-1/FR-027)
- [x] T063 [P] [US5] Provider-asymmetry disclosure + quantified per-provider counts at the shared-rule UI in `src/components/learning/` (RuleCard/StatusStrip) (M-6/FR-028)
- [x] T064 [P] [US5] Multi-model cost-tiebreak regression test in `src-tauri/src/cc_client.rs` test module (multi-entry `modelUsage`; cheap alphabetically-first vs costly primary) (L-2)
- [ ] T065 [US5] Validate quickstart V6 manually

**Checkpoint**: All 5 stories independently functional.

---

## Phase 8: Polish & Cross-Cutting Concerns

- [x] T066 [P] Doc drift (L-1): update `lat.md/{features.md,backend.md,data-flow.md,frontend.md}` for pinned synthesis model + lifecycle/governance/redaction/eval architecture; add an "as-built superseded Haiku" note in `specs/004-quill-native-insights/`
- [x] T067 lat.md sync + `lat check` passes (project post-task gate — REQUIRED)
- [x] T068 SC-011 tuning pass: compare against `specs/005-learning-system-hardening/baseline.md` (T006); confirm learning value preserved; tune `min_eligibility`/`min_evidence_count`/entropy threshold against the recorded sample
- [x] T069 [P] Final `cargo fmt` + `cargo clippy --all-targets -D warnings` clean; drop dead `Model::Sonnet`/`Haiku` only if clippy-clean
- [ ] T070 Full quickstart.md run V1–V8 (all SC-001…SC-013)

---

## Dependencies & Execution Order

### Phase dependencies

- **Setup (P1)**: no deps. T006 (SC-011 baseline) MUST complete before any
  Foundational/story task that mutates the rule pipeline (it is the only
  SC-011 anchor; legacy wipe T032 destroys legacy rules).
- **Foundational (P2)**: after Setup. **Blocks US2/US3/US4 and US3's
  feedback/US4 persistence** (migration 25 = research.md C1). US1 also waits on
  Foundational T009 (redaction core).
- **US1 (P3, MVP)**: after Foundational (needs T009 redaction core, T011 for
  sandbox metadata tests).
- **US2 (P4)**: after Foundational (needs T007 schema).
- **US3 (P5)**: after **US2** — T042 reworks the same `write_rule_files` site
  T025 strips (research.md C4); needs T007 schema + T010 scorer + T009 redact
  (citation snapshots) + T038 evidence refs.
- **US4 (P6)**: after US3 (eval consumes grounded evidence + operator feedback)
  and Foundational T011 (inference double). T052/T053 need migration 25.
- **US5 (P7)**: after Foundational; T058/T059 need the `degraded` status from
  US2 T033 (research.md C5 — status + decode together). Otherwise independent.
- **Polish (P8)**: after all targeted stories. T067 `lat check` is mandatory;
  T068 compares against the T006 baseline.

### Critical research.md constraints encoded

- **C1**: migration 25 is one shared transactional migration (T007), landed in
  Foundational before US2/US3/US4.
- **C2**: lifecycle (T026) + tombstone (T027/T028) + reconcile-awareness (T028)
  ship together within US2 — partial landing re-opens C-5.
- **C3**: redaction core (T009, Foundational) precedes US2 citation snapshots
  (T029) and legacy-archive content capture (T032).
- **C4**: T025 (US2, delete auto-write) and T042 (US3, new gate at same site)
  are sequenced US2→US3 on `write_rule_files`.
- **C5**: T033 (`degraded`, US2) precedes T058/T059 (US5 decode/UI).
- **C6**: `min_confidence`→`min_eligibility` — T025 makes the old gate inert,
  T042 redefines the setting; keep coordinated.

### Within each story

Tests (where listed) → schema/model → service/storage → IPC/endpoint → UI →
manual quickstart validation. Story complete before next priority.

### Parallel opportunities

- Setup: T001–T004 and T006 all [P] (T006 independent of the stub/CI/build).
- Foundational: T009 ‖ T011 (T007 must precede T008; T010 independent).
- US1: T015 ‖ T016 ‖ T017 (distinct files) after T009; T012 first.
- US3: T039 ‖ T040 (distinct files); T048 (frontend) ‖ backend T038–T047.
- US4: T054 ‖ T055 ‖ T050 (distinct files).
- US5: T059 ‖ T060 ‖ T063 ‖ T064 (distinct files).
- After Foundational, US1 and US5 (non-feedback parts) can run in parallel with
  US2 by separate developers; US3/US4 are gated as above.

---

## Parallel Example: User Story 1

```bash
# After T009 (redaction core) + T012 (tests authored):
Task: "T015 Stream B redact in src-tauri/src/git_analysis.rs"
Task: "T016 Synthesis context redact in src-tauri/src/learning.rs:997-1028"
Task: "T017 memory_optimizer redact in src-tauri/src/memory_optimizer.rs"
```

---

## Implementation Strategy

### MVP first (US1)

1. Phase 1 Setup (incl. T006 SC-011 baseline) → 2. Phase 2 Foundational
(CRITICAL) → 3. Phase 3 US1 → 4. STOP & VALIDATE quickstart V1+V7 (privacy is
the highest-impact, lowest-tolerance risk; it is the trust precondition for
everything else).

### Incremental delivery

Setup (baseline) → Foundational → US1 (privacy MVP) → US2 (governance: no
autonomous global rule, reversible) → US3 (evidence-grounded + operator
feedback) → US4 (evaluation + CI gate) → US5 (observability). Each story is
independently testable via its quickstart V-section; each adds value without
breaking prior stories.

### Parallel team strategy

After Foundational: Dev A → US1; Dev B → US2 (then US3, which depends on US2);
Dev C → US5 non-feedback parts; US4 follows US3. Converge at Polish (T067
`lat check`, T068 SC-011 tuning vs baseline, T070 full quickstart).

---

## Notes

- Migration is **25** (research.md R-0; plan corrected — `21` would silently
  no-op and PK-collide). T008 asserts it.
- T006 (SC-011 baseline) is the only comparison anchor for SC-011 — it is
  unrecoverable once changes land; do not skip or reorder after Foundational.
- `[P]` = different files, no incomplete-task dependency.
- Test tasks are REQUIRED here (FR-021) — T008/T012/T024/T037/T054/T055/T064
  + the CI gate T056; do not skip.
- T024 includes the N7 assertion (periodic run with autonomous removed writes
  0 `.md`).
- Commit after each task or logical group; stop at any checkpoint to validate.
- T067 (`lat check`) and T070 (full quickstart, SC-001…SC-013) are hard
  completion gates.
