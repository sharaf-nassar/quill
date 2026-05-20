# Implementation Plan: Landlock Inference Sandbox

**Branch**: `007-landlock-inference-sandbox` | **Date**: 2026-05-19 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `specs/007-landlock-inference-sandbox/spec.md`

## Summary

Promote the in-kernel **Landlock LSM** to primary Linux inference confinement. **Keep `bwrap` as a fallback** for hosts where it still works. **Retire `SandboxKind::ProcessOnly`** (the just-shipped unshare wrapper from feature 006 — broken on the same userns-restricted hosts as bwrap, no FS-confinement value either way). Linux chain becomes `Landlock → Bwrap → None`, with an **actionable per-process diagnostic** when the chain falls through to `None` — generic when neither mechanism is available, AppArmor-specific when AppArmor's `restrict_unprivileged_userns` is the detected cause of bwrap failing. Feature 006-A's existing honest-disclosure UI (marker + remediation hint) keeps working over the new mechanism. macOS (`sandbox-exec`) and Windows (`JobObject`) byte-for-byte unchanged. No DB migration. One new approved dependency (`landlock` 0.4.4). Same speckit flow as feature 006 — disjoint-files implementation, integrated 0-warning baseline authoritative, one squashed conventional commit at the end.

## Technical Context

**Language/Version**: Rust (workspace edition, `src-tauri/`); TypeScript + React (Vite, `src/`)
**Primary Dependencies**: Tauri, rusqlite (SQLite), serde/serde_json, tokio, serial_test, tempfile. **New (approved)**: `landlock` v0.4.4 (Apache-2.0/MIT; by the kernel feature's author Mickaël Salaün; ABI v6; Linux 5.13+). Linux-only target-cfg.
**Storage**: SQLite `usage.db`, no migration. Persisted `sandbox` is `Option<String>` with `#[serde(default)]`; feature 006-A's `sandbox_tag_is_fs_confined` already classifies unknown tags conservatively.
**Testing**: `cargo test --lib` with the existing `#[test]`/`#[tokio::test]` + `#[serial]` + `TempDir`/`init_storage_in` harness; the `cc_client` `#[cfg(test)]` `InferenceDoubleGuard` offline scripted-inference double. Frontend `npm run build` for type/render correctness. CI gate = feature-005 FR-021.
**Target Platform**: Linux development host (primitive swap is Linux-only). macOS/Windows confinement code paths byte-for-byte unchanged; the macOS cfg path cannot compile here.
**Project Type**: Desktop app — Rust (Tauri) backend + React frontend.
**Performance Goals**: No measurable change. Landlock setup is a handful of syscalls (open `O_PATH` per RO/RW path + ruleset create + add_rule × N + restrict_self) on a path executed at most a few times per learning run. The diagnostic-emit logic adds one regex/substring check on bwrap stderr only when bwrap is the active mechanism AND it failed; otherwise it's a no-op.
**Constraints**: 0-warning baseline as the authoritative integrated gate — `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`; `npm run build`; `lat check`. Never fail-closed (R-7). Immutability per repo rules. Strict commit hooks.
**Scale/Scope**: Concentrated in `cc_client.rs` (variant change + Landlock setup + diagnostic emitter) and a small touch to `storage.rs` for the classifier addition + decode-deploy-safety test extension.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

`.specify/memory/constitution.md` remains an unratified template. Binding gates are the repo conventions:

| Gate | Status | Notes |
|------|--------|-------|
| lat.md sync + `lat check` | PASS (planned) | `backend.md` sandbox paragraph rewrite; dated reconciliation lines into specs/005 R-7.6 and specs/006 R-A. |
| 0-warning baseline | PASS (planned) | Fmt + forced-clean clippy `-D warnings` + `cargo test --lib` + `npm run build`. |
| FR-021 deterministic tests on the learning surface | PASS (planned) | Mapping totality, SC-013 array extension, decode deploy-safety, Landlock ruleset-construction smoke, diagnostic classifier unit tests. All offline. |
| New dep requires user approval | PASS | `landlock` v0.4.4 explicitly approved during scoping. No other new deps. |
| No DB migration unless justified | PASS | None. |
| Never fail-closed | PASS | Any Landlock/Bwrap setup error falls through to `SandboxKind::None` with an unwrapped spawn and the diagnostic emitted; the run continues. |
| Strict commit hooks | PASS (deferred) | One squashed commit only after the integrated gate. |

**Result**: No violations. Complexity Tracking left empty.

## Project Structure

### Documentation (this feature)

```text
specs/007-landlock-inference-sandbox/
├── plan.md              # This file
├── spec.md              # /speckit-specify output (updated for three-tier)
├── research.md          # Phase 0 design decisions (R-A..R-F)
├── data-model.md        # Phase 1 vocabulary + classifier deltas (no schema change)
├── quickstart.md        # Phase 1 maintainer verification walkthrough
├── contracts/
│   └── landlock-sandbox.md   # Phase 1 confinement-chain contract
├── checklists/
│   └── requirements.md  # spec quality checklist
└── tasks.md             # Phase 2 — /speckit-tasks (NOT created here)
```

### Source Code (repository root)

```text
src-tauri/
├── Cargo.toml                # +landlock 0.4.4 (target.'cfg(target_os = "linux")')
└── src/
    └── cc_client.rs          # SandboxKind: drop ProcessOnly, add Landlock
                              # (keep Bwrap); detect_sandbox_kind probes
                              # Landlock first then bwrap; apply_sandbox
                              # gains Landlock arm (in-process pre-spawn
                              # hook) and keeps the Bwrap arm; new
                              # build_ruleset(); new SandboxDiagnostic
                              # one-shot emitter; bwrap-broken-on-this-host
                              # process-wide latch; AppArmor stderr
                              # classifier.
src-tauri/src/storage.rs      # sandbox_tag_is_fs_confined: add "landlock"
                              # → true; extend decode test for legacy
                              # "bwrap"/"process-only"/"unshare" deploy
                              # safety.

lat.md/
└── backend.md                # "Claude Code Inference Client" sandbox
                              # paragraph rewrite — Landlock primary,
                              # Bwrap fallback, ProcessOnly removed,
                              # actionable diagnostic mentioned.

specs/005-learning-system-hardening/research.md   # dated R-7.6 reconciliation
specs/006-learning-hardening-followups/research.md # dated R-A reconciliation
```

**Structure Decision**: Single focused track concentrated in `cc_client.rs`. No parallel-agent split this time (smaller than feature 006 and the diagnostic / fallback latch state is shared across the additions). One subagent drives the implementation; the integrated 0-warning baseline is authoritative.

---

## Design

### D.1 Problem (precise, recap of evidence already gathered)

- Default Ubuntu 24.04 ships `kernel.apparmor_restrict_unprivileged_userns=1` and no AppArmor profile for `/usr/bin/bwrap`. Bare-bwrap reproduction on the dev host fails with `bwrap: setting up uid map: Permission denied`. The same restriction blocks `/usr/bin/unshare` invocations (not setuid; same `CLONE_NEWUSER` requirement). Current chain `Bwrap → ProcessOnly → None` cannot do FS confinement on this host class.
- Anthropic's own [anthropic-experimental/sandbox-runtime Issue #74](https://github.com/anthropic-experimental/sandbox-runtime/issues/74) (Dec 2025) is the same problem, still open — no upstream answer in our ecosystem.
- [Codex CLI 0.117.0](https://www.jdhodges.com/blog/codex-sandbox-ubuntu-24-04-fix/) added a Landlock-based sandbox opt-in precisely to bypass this class.
- Linux's [Landlock LSM](https://docs.kernel.org/userspace-api/landlock.html) (5.13+, ABI v6 current Mar 2026) is the kernel-blessed answer: in-process, unprivileged-self-restricts, no user namespaces, no AppArmor entanglement, survives `execve` with `PR_SET_NO_NEW_PRIVS`. The Rust crate [`landlock` v0.4.4](https://crates.io/crates/landlock) by the kernel feature's author is mature.
- bwrap remains genuinely useful on hosts where userns is permitted (Debian sid, Fedora without policy restrictions, Ubuntu with the `bwrap-userns-restrict` profile installed). Retaining it as a fallback preserves FS confinement for that audience.

### D.2 Design options & decisions

#### R-A — Which Landlock ABI to target

| Option | Approach | Tradeoffs |
|---|---|---|
| A1 — Pin v1 | Hard-require ABI v1 (5.13) | Smallest surface; gives up truncate/network rights on modern kernels |
| A2 — Hard-require latest | Pin v6 (kernel 6.7) | Excludes Ubuntu 22.04 LTS (5.15 → only ABI v1); fail-closed on supported-but-too-old kernels |
| **A3 — Best-effort highest** (**recommended**) | Declare `ABI::V3` (RO + write + truncate) with `CompatLevel::BestEffort`; the crate auto-degrades available access rights on older kernels | Works on every Landlock kernel (≥5.13); preserves the rights the kernel actually offers; matches feature 006's "degrade and record honestly" philosophy |

**Decision — R-A: A3.** `ABI::V3` + `BestEffort`. If create fails entirely (kernel < 5.13 or Landlock disabled), the crate returns `Err` → caller falls through to the Bwrap arm.

#### R-B — How to apply Landlock before invoking `claude`

| Option | Approach | Tradeoffs |
|---|---|---|
| B1 — Restrict the parent Quill process | Call `restrict_self` in Quill | **Hard no** — would break every other Quill code path |
| B2 — Tiny helper binary `quill-sandbox-launcher` | Helper binary calls restrict_self, then `execve`s claude | New shipped binary; packaging complexity |
| **B3 — Pre-spawn hook in the forked child** (**recommended**) | Build the `RulesetCreated` in the parent (all allocations + the `PathFd` opens done pre-fork). Stash it. Use `std::os::unix::process::CommandExt::pre_exec` on the `tokio::process::Command`'s underlying `std::process::Command`. The closure runs in the forked child between `fork` and the `claude` invocation; inside it: `prctl(PR_SET_NO_NEW_PRIVS, 1, …)`, then `ruleset.restrict_self()`. Both async-signal-safe. | Canonical Unix pattern; matches rust-landlock's `sandboxer` example; in-process; the ruleset binds the child + everything it forks via the no-new-privs flag |

**Decision — R-B: B3.** Build pre-fork (allocations + syscalls allowed; we're not in the forked child yet). The pre-spawn hook closure does only syscalls. The hook is `unsafe` (small block, well-isolated).

#### R-C — Ruleset construction & path policy (matches bwrap's effective rules)

- **RO `path_beneath` rules**: `/usr`, `/bin`, `/sbin`, `/lib`, `/lib32`, `/lib64`, `/etc`, `/opt`, `/nix` (if present). Plus the resolved `claude_install_root` (so Nix / user-prefix installs find their runtime). Same set the bwrap arm currently `--ro-bind-try`s, minus what bwrap manages internally (`/proc`, `/dev`, `/tmp`) that Landlock doesn't.
- **RW `path_beneath` rule**: exactly the per-call `TempDir` (the `out.json` sink). Nothing else.
- **Deny-by-default everywhere else**: no `$HOME`, no `~/.claude`, no `~/.config`, no project trees, no Quill DB.
- **Network**: no Landlock network rules (network preserved). The ABI v4 outbound TCP allowlist for `api.anthropic.com:443` is **out of scope** (feature-008 candidate per spec assumptions).
- **Absent optional paths**: silently skip (`/lib32`, `/nix`, `/opt` commonly missing); not an error.

**Decision — R-C**: a `LandlockPolicy` value object (RO set + RW path + ABI) → `build_ruleset(policy) -> Result<RulesetCreated, BuildError>` private fn. Pure construction; no restrict_self call here.

#### R-D — Three-tier fallback chain and per-process latching

When does each tier apply?

1. **Landlock** (detection-time probe — cheap, in-process ruleset attempt → discard). If supported → use it; record `SandboxKind::Landlock`.
2. **Bwrap** (PATH probe). If installed → use it; record `SandboxKind::Bwrap`. (Invocation may still fail at spawn time on AppArmor-restricted hosts → see below.)
3. **None** (no wrapper). Record `SandboxKind::None`. Emit the one-shot diagnostic per R-F.

**Bwrap invocation-time failure handling.** The current code surfaces a non-zero bwrap exit as `InferenceError::Spawn(stderr)`. New behavior:

- Parse the bwrap stderr for the AppArmor / userns-restriction signature (substring match — see R-F).
- **Latch a per-process `bwrap_broken_on_this_host: AtomicBool`** (or `OnceLock<BwrapBrokenCause>` carrying the classified cause). Once set, subsequent calls within this Quill process **skip bwrap entirely** and go straight to None.
- Emit the appropriate one-shot diagnostic on first detection.
- The first call that triggered the latch records `sandbox: "bwrap"` (the *attempted* mechanism, per feature 006-A's "record what the host probe said even on failure" — preserving SC-013) and `failure_kind: "spawn"`. The run as a whole degrades to "no streams produced findings" only if ALL streams hit the latch on first call; in practice the latch trips on stream A and streams B/C of the same run already see the latch set and go unwrapped → they can now spawn and produce findings.

(Note: the run-49 failure pattern was "all three streams fail at spawn." With latching, only the first stream to hit the failure pays the cost; the rest proceed unwrapped. That alone changes the user-visible behavior from "0 findings → run failed" to "the operator gets a real diagnostic + 2/3 streams completed".)

**Decision — R-D**: detection-time chain is Landlock → Bwrap → None. Bwrap-invocation latching + AppArmor classifier as described. No retries inside a single call — if the first attempt fails, the latch is set and that call's result stands; subsequent calls use the now-known-broken state.

#### R-E — How to verify the diagnostic in tests (no kernel involved)

The diagnostic classifier is a pure function:

```text
classify_bwrap_failure(stderr: &str) -> BwrapBrokenCause
```

where the cause enum is `AppArmorRestrictUserns | Other`. Substring matching on the two known bwrap-on-Ubuntu-24.04 signatures:

- `"setting up uid map: Permission denied"` → `AppArmorRestrictUserns`
- `"loopback: Failed RTM_NEWADDR: Operation not permitted"` → `AppArmorRestrictUserns` (Codex blog signature; we don't currently use `--unshare-net` but be tolerant)
- anything else → `Other`

Pure unit test: feed each signature, assert the classification. No `#[serial]` needed; no spawning; no kernel.

**Decision — R-E**: `classify_bwrap_failure` is a private `fn(&str) -> BwrapBrokenCause`. Unit-tested with a handful of synthetic stderr strings (including a few real ones captured from the bare-bwrap reproduction and the Codex blog).

#### R-F — Diagnostic emission: where, when, what

**Where**: two surfaces.

1. **stderr via `log::error!`** — picked up by Tauri's logger and printed in the `tauri dev` terminal. The operator running `tauri dev` sees it within seconds of triggering the broken run.
2. **The per-run `learning_runs.logs` column** — already populated by `learning.rs` from the per-stream `LogEvent`s. We push the diagnostic into the per-call log channel that ends up in this column; visible in run-history detail.

**When**: once per Quill process. A `OnceLock<()>` guards emission so a multi-stream run that hits the latch only logs the diagnostic for stream A; B and C are silent.

**What**: two message templates.

- **Generic (FR-014)** — emitted when both mechanisms are unavailable at *detection* (Landlock probe failed + bwrap not on PATH):

  > Filesystem confinement is unavailable on this host. Quill will continue running learning analyses unwrapped (each per-call inference processes untrusted captured content with full read access to your home directory, ~/.claude credentials, and project trees). To restore filesystem confinement, install **either** a Linux kernel ≥ 5.13 with Landlock LSM enabled (the in-process primitive Quill prefers), **or** the `bubblewrap` package (the subprocess fallback Quill keeps for older kernels and non-Landlock distros). See run-history detail for the per-call recorded state.

- **AppArmor-specific (FR-015)** — emitted when bwrap was attempted and failed with the userns-restriction signature:

  > Filesystem confinement is unavailable on this host because AppArmor's `restrict_unprivileged_userns` policy is blocking bubblewrap's user-namespace creation (detected from bwrap stderr "<the captured signature>"). Quill will continue running learning analyses unwrapped. To restore confinement, either:
  > 1. (Recommended) Install an AppArmor profile that grants `userns,` to `/usr/bin/bwrap`. Ubuntu 24.04.02+ ships this as the `bwrap-userns-restrict` package; on stock 24.04, create `/etc/apparmor.d/bwrap` with the profile body documented at <link> and run `sudo apparmor_parser -r /etc/apparmor.d/bwrap`.
  > 2. Install a Linux kernel that supports Landlock LSM (any 5.13+; Ubuntu 22.04+ already does — if Landlock isn't being detected on your 5.13+ kernel, that's a separate issue worth filing).
  > 3. (Not recommended; weakens system hardening) `sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0` and persist in `/etc/sysctl.d/`.

The exact wording is in `contracts/landlock-sandbox.md` so the test can string-match a stable substring without coupling to prose.

**Decision — R-F**: two-message vocabulary; one-shot per process via `OnceLock`; emitted to both `log::error!` and the per-call log channel; stable substrings checkable by unit tests.

### D.3 Test strategy (FR-021 deterministic, offline)

The hard constraint: **we cannot call `restrict_self` in any test** — it would permanently restrict the test process's FS access. So:

- **Pure mapping totality** (extend `sandbox_kind_as_str_and_fs_confinement_mapping_is_total`): drop the `ProcessOnly` case, **keep** the `Bwrap` case, add `Landlock` (`as_str()` = `"landlock"`, classifier → `true`). The closed write set becomes `{"landlock", "bwrap", "sandbox-exec", "job-object", "none"}`.
- **SC-013 metadata-recorded** (extend `sandbox_metadata_is_recorded_for_every_call`): update `ALL` and Linux `platform_expected` to `{"landlock", "bwrap", "none"}`. Keep host-agnostic membership/classification assertions only.
- **Decode deploy-safety** (extend `decode_inference_metadata_is_tolerant_and_folds_rollup`): historical `"bwrap"` → `fs_confined=true`, historical `"process-only"` → `false`, historical `"unshare"` → `false`, new `"landlock"` → `true`, unknown tag → `false`. Raw tag preserved verbatim.
- **Ruleset construction smoke** (new test in `cc_client.rs`): build `LandlockPolicy` with default RO set + a `TempDir` as RW; call `build_ruleset`; assert `Ok(_)` on any Landlock-supported host (gate the test with `#[ignore]` if the host probe fails — same pattern as a couple of existing host-dependent tests). Second test: deliberately include a non-existent path (e.g. `/nix-not-here`); assert it's skipped silently and the build still succeeds.
- **Diagnostic classifier unit tests**: feed each known AppArmor signature (the dev-host stderr + the Codex-blog stderr) → assert `AppArmorRestrictUserns`. Feed a synthetic "wrong file format" stderr → assert `Other`. Pure; no `#[serial]`.
- **Diagnostic one-shot emission test**: stub-set the `OnceLock`, simulate two successive failure events through the emitter, assert only one `log::error!` line was produced (use `log::set_max_level`/`set_logger` pattern with a capturing test logger, or just call the emitter twice and observe the latch state).
- **InferenceDoubleGuard offline path** unchanged: the scripted-response double bypasses `apply_sandbox`; confirm `doubled_metadata` records the detected kind correctly under the new chain.
- **Frontend**: no new test infra (T012 stays deferred from feature 006). `npm run build` (tsc) covers type wiring.

### D.4 lat.md sync points

- `lat.md/backend.md` "Claude Code Inference Client" sandbox paragraph (currently ~lines 355–368): rewrite. State the three-tier Linux chain (Landlock → Bwrap → None), the actionable diagnostic at the falls-through-to-None step, and the fact that the ProcessOnly tier is gone. macOS/Windows paragraphs unchanged.
- `specs/005-learning-system-hardening/research.md` R-7 / R-7.6: append dated reconciliation line (2026-05-19) — feature 007 introduces Landlock as primary and demotes bwrap to fallback; the "best-available OS-level confinement" hierarchy is preserved, just with a better top tier.
- `specs/006-learning-hardening-followups/research.md` R-A: append dated reconciliation line (2026-05-19) — feature 007 retires the `ProcessOnly` variant that feature 006-A introduced; the honest-tag classifier `sandbox_tag_is_fs_confined` is built upon (just adds `"landlock"`→true); the RunHistory disclosure UI is reused.
- `lat check` MUST pass on the integrated baseline.

---

## Phase 2 — Dependency-ordered task list (build sequence)

Formalized via `/speckit-tasks` only after plan approval. **Single track.**

1. **T001 Setup**: confirm branch `007-…`; pre-change green baseline.
2. **T002 Dep add**: add `landlock = "0.4.4"` to `src-tauri/Cargo.toml` under `[target.'cfg(target_os = "linux")'.dependencies]`.
3. **T003 SandboxKind contraction**: in `cc_client.rs::SandboxKind`, remove `ProcessOnly` (added in feature 006 — gone in this feature); **keep** `Bwrap`; add `Landlock`. Update `as_str` (closed set → `{"landlock","bwrap","sandbox-exec","job-object","none"}`); update enum doc.
4. **T004 Classifier extension**: in `cc_client.rs::sandbox_tag_is_fs_confined`, add `"landlock"` to the FS-confined match alongside `"bwrap"` and `"sandbox-exec"`. Closed-set forward-compatibility preserved.
5. **T005 Detection chain rewrite**: `detect_sandbox_kind` Linux branch → probe Landlock first (try a tiny ruleset create, discard); if supported → `Landlock`. Else probe bwrap on PATH; if present → `Bwrap`. Else → `None`. Cache process-wide (existing pattern).
6. **T006 LandlockPolicy + build_ruleset**: new value-type + private fn. Default policy = RO {/usr,/bin,/sbin,/lib,/lib32,/lib64,/etc,/opt,/nix-if-present, claude_install_root} + RW {per-call TempDir} + ABI::V3 + BestEffort. Skip-absent-paths logic.
7. **T007 apply_sandbox arms**: **delete** `ProcessOnly` arm + its unshare wrapper helpers. **Keep** the Bwrap arm intact. **Add** Landlock arm: build policy → `build_ruleset` → install via the `tokio::process::Command`'s underlying `pre_exec` hook that runs `prctl(PR_SET_NO_NEW_PRIVS)` + `ruleset.restrict_self()`. On `build_ruleset` Err, fall through to bwrap arm (per the detection chain decision — the caller knows the host claims Landlock from detection, but if build fails for any reason, behave as if Landlock is unavailable).
8. **T008 Diagnostic infrastructure**:
   - New private enum `BwrapBrokenCause { AppArmorRestrictUserns, Other }`.
   - New private fn `classify_bwrap_failure(stderr: &str) -> BwrapBrokenCause`.
   - New private `OnceLock<()>` for one-shot diagnostic emission.
   - New private `OnceLock<BwrapBrokenCause>` for the bwrap-broken-on-this-host latch.
   - New private fn `emit_no_confinement_diagnostic(cause: Option<BwrapBrokenCause>, target_log_channel: …)` that, if the OnceLock is unset, writes the appropriate template to both `log::error!` and the per-call log channel; sets the OnceLock.
   - Wire emission at: (a) detection-time when `detect_sandbox_kind` returns `None` and bwrap is also not on PATH (`cause = None` → generic FR-014 template); (b) bwrap-invocation failure when the classifier returns `AppArmorRestrictUserns` (`cause = Some(AppArmor…)` → specific FR-015 template); (c) bwrap-invocation failure with `Other` cause → generic FR-014 template plus the captured stderr appended.
9. **T009 Bwrap latching at invocation**: in `invoke_raw`, when the active mechanism is Bwrap and the spawn errors with `InferenceError::Spawn(stderr)`, classify the stderr, set the latch, emit the diagnostic. Subsequent calls in this Quill process see the latch and behave as if `SandboxKind::None` was detected from the start (no bwrap spawn attempted; record `sandbox: "none"`).
10. **T010 Tests — closed-set + SC-013 arrays**: update `sandbox_metadata_is_recorded_for_every_call` arrays + the loop variants; update `sandbox_kind_as_str_and_fs_confinement_mapping_is_total` cases. Keep `Bwrap` recognized; drop `ProcessOnly`; add `Landlock`.
11. **T011 Tests — new**:
    - `build_ruleset_succeeds_with_default_policy_on_landlock_host` (gated by host probe — same pattern as a couple of existing tests).
    - `build_ruleset_skips_absent_optional_paths_without_error`.
    - `classify_bwrap_failure_detects_apparmor_userns_signature` (multiple inputs).
    - `classify_bwrap_failure_returns_other_for_unrelated_stderr`.
    - `emit_no_confinement_diagnostic_is_one_shot_per_process` (the latch test).
12. **T012 Decode deploy-safety extension**: in `storage.rs::decode_inference_metadata_is_tolerant_and_folds_rollup`, add new cases for historical `"bwrap"` (→ `fs_confined=true`), historical `"process-only"` (→ `false`), historical `"unshare"` (→ `false`), new `"landlock"` (→ `true`).
13. **T013 lat.md sync**: rewrite `lat.md/backend.md` "Claude Code Inference Client" sandbox paragraph; append dated reconciliation lines into `specs/005-learning-system-hardening/research.md` and `specs/006-learning-hardening-followups/research.md`. `lat check` must pass.
14. **T014 Integrated baseline (authoritative)**: from repo root — `cargo fmt --check`; `cargo clean -p quill && cargo clippy --all-targets -- -D warnings`; `cargo test --lib`; `npm run build`; `lat check`. All must pass with zero new warnings.
15. **T015 V-acceptance (manual)**: real Ubuntu 24.04 run on this host — trigger a learning run; verify the latest `learning_runs` row shows `status='completed'`, every per-call `sandbox: "landlock"`, no `failure_kind: "spawn"`, run-history UI shows no remediation marker. Also: simulate a Landlock-disabled host (e.g. via a feature-flag or by temporarily masking the probe) → verify the bwrap fallback runs, and if bwrap is AppArmor-blocked (the actual current state of this host before any profile install) → verify the AppArmor-specific diagnostic appears in the run log and stderr.
16. **T016 Squashed commit**: one bare `git commit` per the strict hook rules — **only on explicit user go-ahead after T014/T015 pass**.

## Complexity Tracking

No constitution violations; section intentionally empty.
