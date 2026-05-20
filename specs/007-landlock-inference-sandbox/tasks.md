---
description: "Task list for Landlock Inference Sandbox"
---

# Tasks: Landlock Inference Sandbox

**Input**: Design documents from `/specs/007-landlock-inference-sandbox/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/landlock-sandbox.md, quickstart.md

**Tests**: Test tasks are INCLUDED — explicitly required by spec FR-012/FR-021 (CI-gated learning surface; deterministic unit tests on the existing harness).

**Organization**: Grouped by user story. US1 = Landlock primary FS confinement (P1). US2 = Actionable diagnostic when chain falls through (P2). US3 = Historical decode deploy-safety (P3). The work is concentrated in `src-tauri/src/cc_client.rs` (US1 + US2 share state — the bwrap-broken latch and the diagnostic emitter); `src-tauri/src/storage.rs` carries the classifier extension + the decode-test extension (US3) and is the one genuinely-parallelizable surface.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: US1 (Landlock primary) · US2 (diagnostic) · US3 (decode deploy-safety)

## Path Conventions

Desktop app: Rust backend `src-tauri/src/`, knowledge graph `lat.md/`, specs `specs/007-landlock-inference-sandbox/`.

---

## Phase 1: Setup (Shared)

**Purpose**: Confirm clean starting baseline + add the one approved new dependency.

- [ ] T001 Confirm branch `007-landlock-inference-sandbox` is checked out and the working tree contains only the planning changes from this command sequence (`specs/007-...`, `.specify/feature.json`, `CLAUDE.md` SPECKIT block)
- [ ] T002 Establish the pre-change green baseline from repo root: `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`; `npm run build`; `lat check` — record that all pass before any code change
- [ ] T003 Add `landlock = "0.4.4"` to `src-tauri/Cargo.toml` under `[target.'cfg(target_os = "linux")'.dependencies]` so macOS/Windows builds are unchanged; run `cargo build` from `src-tauri/` to fetch + verify version pin

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared type-level changes that ALL three user stories depend on. No story label since they're load-bearing for the whole feature.

- [ ] T004 In `src-tauri/src/cc_client.rs::SandboxKind`: remove the `ProcessOnly` variant (introduced in feature 006-A, retired here); keep `Bwrap`/`SandboxExec`/`JobObject`/`None`; add `Landlock` variant with doc explaining it is in-process kernel-LSM FS confinement; update the closed `as_str()` vocabulary to `{"landlock","bwrap","sandbox-exec","job-object","none"}`; update enum-level doc
- [ ] T005 In `src-tauri/src/cc_client.rs::sandbox_tag_is_fs_confined`: add `"landlock"` to the FS-confined `matches!` arm alongside `"bwrap"` and `"sandbox-exec"` (keep existing entries for legacy decode forward-compat). Pure `matches!`; one-line change
- [ ] T006 Confirm the existing test substrate is intact: `cc_client.rs` exposes the `#[cfg(test)]` `set_inference_double_scoped` / `InferenceDoubleGuard` seam (US1+US2 substrate); `storage.rs` exposes `init_storage_in`/`TempDir`/`#[serial]`/`clear_env()` used by `decode_inference_metadata_is_tolerant_and_folds_rollup` (US3 substrate). No edit if already present — this is a verification step

**Checkpoint**: Foundational complete → US1, US2, US3 can proceed (US3 is independent of US1/US2 and can run in parallel with them).

---

## Phase 3: User Story 1 — Landlock primary FS confinement (Priority: P1)

**Goal**: On a default modern Linux host (Ubuntu 22.04+ / 24.04+, current Fedora, etc., no manual OS configuration), learning runs complete with real filesystem confinement applied via Landlock LSM in-process; the recorded per-call `sandbox` tag is `"landlock"`.

**Independent Test**: On any Landlock-supported host, trigger a learning run; verify every per-call inference metadata record carries `sandbox: "landlock"` and `fs_confined: true`; the run-history UI does not show the not-FS-confined marker.

### Implementation (US1)

- [ ] T007 [US1] In `src-tauri/src/cc_client.rs`: add a private `LandlockPolicy` value-type carrying RO path set (`/usr`, `/bin`, `/sbin`, `/lib`, `/lib32`, `/lib64`, `/etc`, `/opt`, `/nix` if present, plus the resolved `claude_install_root`), the RW path (per-call `TempDir`), and the ABI choice (`ABI::V3`). Provide a `default_for_call(rw_dir: &Path, claude_path: &Path) -> LandlockPolicy` constructor
- [ ] T008 [US1] In `src-tauri/src/cc_client.rs`: add a private function `build_ruleset` taking a `&LandlockPolicy` and returning `Result<RulesetCreated, BuildError>`. It opens `PathFd`s for every present RO/RW path (silently skipping absent optional paths), creates the `Ruleset` with `CompatLevel::BestEffort` and `AccessFs::from_all(policy.abi)`, adds the `path_beneath_rules` for RO and RW sets, and returns the built `RulesetCreated`. **Does not** call `restrict_self`
- [ ] T009 [US1] In `src-tauri/src/cc_client.rs::detect_sandbox_kind` Linux branch: rewrite the probe order. First, attempt a tiny in-process Landlock probe (build a minimal ruleset and discard); if it succeeds → return `SandboxKind::Landlock`. Else fall through to the existing bwrap PATH probe → `SandboxKind::Bwrap`. Else `SandboxKind::None`. Cache the result process-wide (existing pattern). macOS/Windows cfg branches unchanged
- [ ] T010 [US1] In `src-tauri/src/cc_client.rs::apply_sandbox` Linux branch: **add** the Landlock arm. Build `LandlockPolicy::default_for_call(rw_dir, claude_path)`; call `build_ruleset`. On `Ok(ruleset)`: take the existing `tokio::process::Command`, reach its `std_command_mut()`, and install — inside an `unsafe` block — the pre-spawn hook via `CommandExt::pre_exec` (the function from `std::os::unix::process`). The hook closure must do exactly two async-signal-safe syscalls in order: `prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)` then `ruleset.restrict_self()`. Map any error in either step to `io::Error::other`. Return `(cmd, SandboxKind::Landlock)`. On `Err(_)` from `build_ruleset`: fall through to the existing Bwrap arm (do NOT delete or modify the Bwrap arm — feature 006-A's bwrap fallback stays intact). Keep the macOS `SandboxExec` arm and Windows `JobObject` arm byte-for-byte unchanged
- [ ] T011 [US1] Verify the `claude_install_root` lookup that the bwrap arm already uses is now ALSO consumed by `LandlockPolicy::default_for_call` (single source of truth — extract a private helper if needed; do not duplicate the lookup logic)

### Tests (US1)

- [ ] T012 [P] [US1] Add a host-gated test in `src-tauri/src/cc_client.rs` (`#[test]` + `#[cfg(target_os = "linux")]`, gated to early-`return Ok(())` if the host Landlock probe fails — same pattern as any existing host-dependent test): `build_ruleset_succeeds_with_default_policy_on_landlock_host` — call `default_for_call(&tempdir, &resolved_claude_path)`, build, assert `Ok(_)`. Does NOT call `restrict_self`
- [ ] T013 [P] [US1] Add `build_ruleset_skips_absent_optional_paths_without_error` in `src-tauri/src/cc_client.rs`: construct a `LandlockPolicy` whose RO set deliberately includes a non-existent path (e.g. `/nix-does-not-exist-007`); assert `build_ruleset` returns `Ok(_)` — the absent path is silently skipped, not an error
- [ ] T014 [US1] In `src-tauri/src/cc_client.rs::sandbox_kind_as_str_and_fs_confinement_mapping_is_total`: drop the `ProcessOnly` case from the `cases` table; add `(SandboxKind::Landlock, "landlock", true)`; update the `CLOSED_SET` array to `{"landlock","bwrap","sandbox-exec","job-object","none"}`. Test is otherwise pure — no `#[serial]`
- [ ] T015 [US1] In `src-tauri/src/cc_client.rs::sandbox_metadata_is_recorded_for_every_call`: update the `ALL` constant to the new closed set; update the Linux `platform_expected` to `&["landlock", "bwrap", "none"]`; update the `for k in [...]` enumeration to drop `SandboxKind::ProcessOnly` and add `SandboxKind::Landlock`. Keep host-agnostic membership/classification assertions only

**Checkpoint US1**: Per-track verify — `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`. On this Ubuntu 24.04 host (Landlock supported) the per-call `sandbox: "landlock"` recording is the success signal.

---

## Phase 4: User Story 2 — Actionable diagnostic when chain falls through (Priority: P2)

**Goal**: When Landlock is unsupported AND bwrap is unavailable or invocation-blocked by AppArmor, the operator sees a clear, cause-specific diagnostic in the `tauri dev` terminal stderr and in the run-history detail's `learning_runs.logs` column; subsequent calls in the same Quill process skip the known-broken bwrap (no re-spawn cost).

**Independent Test**: Force the chain to fall through (Landlock-disabled via test seam OR bwrap-absent via PATH masking); verify the appropriate diagnostic substring is emitted exactly once per process and bwrap is not re-attempted within the same process after a latched failure.

### Implementation (US2)

- [ ] T016 [US2] In `src-tauri/src/cc_client.rs`: add a private enum `BwrapBrokenCause { AppArmorRestrictUserns, Other }` with `Debug`/`Clone`/`Copy`. Add a pure private fn `classify_bwrap_failure(stderr: &str) -> BwrapBrokenCause` that substring-matches `"setting up uid map: Permission denied"` and `"loopback: Failed RTM_NEWADDR: Operation not permitted"` → `AppArmorRestrictUserns`; everything else → `Other`. Doc-comment the rationale + cite the contract `C-E1`
- [ ] T017 [US2] In `src-tauri/src/cc_client.rs`: add a process-wide latch `static BWRAP_BROKEN_ON_THIS_HOST: OnceLock<BwrapBrokenCause>` and a one-shot diagnostic guard `static NO_CONFINEMENT_DIAGNOSTIC_EMITTED: OnceLock<()>`. Add a private fn `emit_no_confinement_diagnostic(cause: Option<BwrapBrokenCause>, captured_stderr: Option<&str>, log_chan: &Sender<LogEvent>)` that — if `NO_CONFINEMENT_DIAGNOSTIC_EMITTED.set(()).is_ok()` — formats the diagnostic per cause (Some(AppArmor) → FR-015 template with the AppArmor-specific remediation including the 5-line profile body from the contract; None or Some(Other) → FR-014 generic template) and emits it to BOTH `log::error!` AND the per-call `log_chan` (so it lands in `learning_runs.logs`). Verify the stable substrings from `contracts/landlock-sandbox.md` C-E3 are present in each template
- [ ] T018 [US2] In `src-tauri/src/cc_client.rs::detect_sandbox_kind` Linux branch: when the chain falls all the way to `SandboxKind::None` (Landlock unsupported AND bwrap not on PATH at probe time), call `emit_no_confinement_diagnostic(None, None, &log_chan)` so the generic FR-014 diagnostic is emitted on the FIRST call after the unsupported host is detected. Plumb the log_chan via a thread-local or a parameter; reuse the existing per-call log channel where possible
- [ ] T019 [US2] In `src-tauri/src/cc_client.rs::invoke_raw` (or wherever the bwrap spawn error is currently surfaced as `InferenceError::Spawn`): when the recorded sandbox was `Bwrap` and the spawn returned non-zero, classify the captured stderr via `classify_bwrap_failure`, store the result in `BWRAP_BROKEN_ON_THIS_HOST` (via `OnceLock::set`; ignore the already-set Err — first writer wins), and call `emit_no_confinement_diagnostic(Some(cause), Some(stderr), &log_chan)`. **Before** spawning bwrap in subsequent calls within this process, check the latch via `OnceLock::get`; if set, skip the bwrap wrapping entirely and record `SandboxKind::None`. Do NOT alter the failure record of the FIRST call — it still records `sandbox: "bwrap"`/`failure_kind: "spawn"` per feature 006-A's "record what was attempted" rule

### Tests (US2)

- [ ] T020 [P] [US2] Add `classify_bwrap_failure_detects_apparmor_userns_signature` in `src-tauri/src/cc_client.rs`: feed each known signature (the dev-host one + the Codex-blog one) and assert `AppArmorRestrictUserns`. Pure; no `#[serial]`
- [ ] T021 [P] [US2] Add `classify_bwrap_failure_returns_other_for_unrelated_stderr` in `src-tauri/src/cc_client.rs`: feed synthetic unrelated stderr (e.g. "bwrap: cannot find `claude` on PATH") and assert `Other`
- [ ] T022 [US2] Add `emit_no_confinement_diagnostic_is_one_shot_per_process` in `src-tauri/src/cc_client.rs` (`#[test]` + `#[serial]` since the latch is a process-global): clear the `OnceLock`s via a `#[cfg(test)]` helper if available, else just verify behavior by calling the emitter twice and observing that only the first call has visible effect (capture log output via a test logger or just verify the latch transition). Confirm the FR-015 stable substring appears for `Some(AppArmor)` and the FR-014 substring appears for `None`/`Some(Other)`

**Checkpoint US2**: With US1 already landed, on this Ubuntu 24.04 host the Landlock probe succeeds, so US2 doesn't visibly trigger on the happy path. Verification under a Landlock-disabled test seam (per the V-acceptance scenarios below) is what exercises US2.

---

## Phase 5: User Story 3 — Historical decode deploy-safety (Priority: P3)

**Goal**: Existing recorded `sandbox` tags from prior versions (feature 005 wrote `"bwrap"`/`"sandbox-exec"`/`"job-object"`/`"none"`; feature 006-A also wrote `"process-only"`; pre-feature-006 may carry `"unshare"`; unknown future tags possible) continue to decode without error and classify correctly forever.

**Independent Test**: Seed `inference_metadata` JSON with each historical and unknown tag; decode; assert no error, raw tag preserved verbatim, `fs_confined` classification matches the historical meaning per contract C-D3.

### Implementation (US3)

- [ ] T023 [P] [US3] Extend `decode_inference_metadata_is_tolerant_and_folds_rollup` in `src-tauri/src/storage.rs` with deploy-safety cases:
  - Historical `"bwrap"` (single-call array) → decoded `confinement.fs_confined == true`; raw tag preserved verbatim
  - Historical `"process-only"` (single-call array; the feature 006-A vocabulary now retired) → `fs_confined == false`; raw tag verbatim
  - Historical `"unshare"` (pre-feature-006 legacy; already covered conceptually by feature 006-A's tolerant decode) → `fs_confined == false`; raw tag verbatim
  - New `"landlock"` (current vocabulary) → `fs_confined == true`; raw tag verbatim
  - Already-existing unknown-tag and missing-tag cases stay covered — no behavioral change there
  This task is purely additive to an existing `#[test] #[serial]` so it has no new ordering risk. Marked `[P]` because it touches `storage.rs` only and has no dependency on the cc_client.rs implementation tasks (T004 / T005 add the positive side; T023 verifies it across the full historical tag set)

**Checkpoint US3**: `cargo test --lib decode_inference_metadata_is_tolerant_and_folds_rollup` passes with the extended assertions. The classifier change in T005 is the only production code US3 depends on (already done in Foundational).

---

## Phase 6: Polish & Cross-Cutting (Integrated)

**Purpose**: lat.md sync, authoritative integrated baseline, manual V-acceptance, single squashed commit.

- [ ] T024 [P] Rewrite `lat.md/backend.md` "Claude Code Inference Client" sandbox paragraph (currently ~lines 355-368): state the three-tier Linux chain (`Landlock → Bwrap → None`), describe Landlock as the in-process LSM primary mechanism applied via a forked-child pre-spawn hook, note that the `unshare`-based `ProcessOnly` tier is retired, and describe the actionable diagnostic emitted at the falls-through-to-None step. Update any wiki links pointing at removed items (`SandboxKind::ProcessOnly`). macOS/Windows paragraphs unchanged. `[P]` because it's lat.md only and doesn't depend on test-pass status — can be drafted alongside the test work
- [ ] T025 [P] Append a dated (2026-05-19) reconciliation line to `specs/005-learning-system-hardening/research.md` R-7 / R-7.6 noting that feature 007 introduces Landlock as the primary Linux mechanism in front of bwrap; the "best-available OS-level confinement" hierarchy is preserved; only the top tier changes. Do NOT rewrite history — append only, mirroring feature 006's reconciliation style. `[P]` (no code dependency)
- [ ] T026 [P] Append a dated (2026-05-19) reconciliation line to `specs/006-learning-hardening-followups/research.md` R-A noting that feature 007 retires the `ProcessOnly` variant introduced in 006-A (theatrical on the same userns-restricted hosts as bwrap) but keeps and builds upon the honest-tag classifier `sandbox_tag_is_fs_confined` and the RunHistory disclosure UI. Append only. `[P]`
- [ ] T027 Run the AUTHORITATIVE integrated 0-warning baseline from repo root: `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`; `npm run build`; `lat check` — all five must pass with zero new warnings. This gate is authoritative; per-checkpoint results are advisory
- [ ] T028 Execute `quickstart.md` manual V-acceptance — three scenarios on this Ubuntu 24.04 host: (1) happy-path Landlock primary works (current host has Landlock support); (2) forced Landlock-disabled + bwrap-blocked-by-AppArmor reproduces today naturally → diagnostic emitted; (3) forced Landlock-disabled + bwrap PATH-masked → generic diagnostic emitted. Record observations in a brief acceptance note
- [ ] T029 Create one squashed conventional commit on branch `007-landlock-inference-sandbox` — single bare `git commit` with literal `-m` only, ≤72-char `feat(...)` subject, wrapped body, no AI-attribution lines. Only after T027/T028 pass. Per-user-pattern: do not push or merge; report and await `s` to squash to main

---

## Dependencies & Execution Order

- **Setup (T001-T003)** → blocks everything.
- **Foundational (T004-T006)** → must complete before any user-story phase begins.
- **US1 (T007-T015)**: within-story, T007 → T008; T009 (detection) and T010 (apply_sandbox) depend on T007/T008; T011 is independent cleanup. Tests T012/T013 `[P]` after T008; T014/T015 sequential within `cc_client.rs` test mod (no `[P]` since the array updates share file context).
- **US2 (T016-T022)**: T016 (classifier) and T017 (latch + emitter scaffolding) can land in any order; T018 (detection-time emission) needs T017; T019 (invocation-time emission + latch) needs T016+T017. Tests T020/T021 `[P]` (pure, different test bodies in same file); T022 sequential.
- **US3 (T023)**: entirely independent of US1/US2 implementation; only depends on the Foundational classifier change (T005). **Can run in parallel with all of US1/US2.**
- **Polish (T024-T029)**: T024/T025/T026 `[P]` (different files, drafted alongside code work). T027 strictly after all US tasks. T028 after T027. T029 only after T027/T028 pass + explicit user go-ahead.

## Parallel Execution Strategy

US3 (T023, in `storage.rs`) is the only genuinely-disjoint code track and can run in parallel with US1+US2 (both concentrated in `cc_client.rs`). lat.md sync tasks (T024-T026) are documentation and can be drafted concurrently with any implementation. **One focused subagent** drives the cc_client.rs work (US1 then US2 — they share the latch + diagnostic state so serial-within-file is safer than splitting); a **second subagent** in parallel can drive T023 (US3 decode test) + T024-T026 (lat.md sync). After both join, T027 is the authoritative gate.

## Implementation Strategy

- **MVP** = US1 alone (P1, Landlock primary) — restores FS confinement on this Ubuntu 24.04 host and any modern Linux. Independently shippable.
- **Increment 2** = US2 (P2, actionable diagnostic) — value when something is wrong; orthogonal to US1 working correctly.
- **Increment 3** = US3 (P3, deploy-safety) — protects existing run-history audit fidelity; tax-not-feature but easy to land.
- All three together are scoped as one squashed commit since they share the type-level changes (Foundational phase) and the audit/disclosure story.
