# Quickstart — Maintainer Verification (Feature 007)

How to verify the Landlock primary + bwrap fallback + actionable diagnostic after implementation. No production code lands until this plan is approved.

## Automated baseline (authoritative, post-implementation)

From repo root on the integrated branch:

```bash
cargo fmt --check
cargo clean -p quill && cargo clippy --all-targets -- -D warnings   # forced clean re-lint
cargo test --lib
npm run build                                                       # frontend tsc + build
lat check                                                           # wiki links + code refs
```

All five MUST pass with **zero new warnings**. `cargo check` alone is insufficient (it doesn't run clippy; cached clippy hides regressions — hence the forced clean).

## Automated test surface (FR-021 deterministic, offline)

- `sandbox_kind_as_str_and_fs_confinement_mapping_is_total` (extended) — new closed set + Landlock case.
- `sandbox_metadata_is_recorded_for_every_call` (extended) — `ALL` and Linux `platform_expected` updated.
- `decode_inference_metadata_is_tolerant_and_folds_rollup` (extended) — new cases: `"landlock"`→true, legacy `"bwrap"`/`"process-only"`/`"unshare"` decode correctly, unknown tag → false; raw tag preserved verbatim.
- `build_ruleset_succeeds_with_default_policy_on_landlock_host` (new; gated `#[ignore]` if host probe fails — same pattern as a couple of existing host-dependent tests).
- `build_ruleset_skips_absent_optional_paths_without_error` (new).
- `classify_bwrap_failure_*` (new pure unit tests for both AppArmor signatures + an unrelated stderr).
- `emit_no_confinement_diagnostic_is_one_shot_per_process` (new; latch test).

## Manual V-acceptance on this Ubuntu 24.04 host

Three distinct scenarios are naturally reproducible here:

1. **Landlock primary (expected on any modern Linux):**
   - Trigger a learning run.
   - Check the latest `learning_runs` row via `python3 -c 'import sqlite3; …' on the usage.db file.
   - Expect: `status='completed'`; every per-call `sandbox: "landlock"`; no `failure_kind: "spawn"`; the run-history UI shows no remediation marker.

2. **Forced Landlock-unavailable + bwrap-blocked-by-AppArmor (the run-49 host config):**
   - Temporarily mask the Landlock probe (via a test-only feature flag, or by running on a kernel where Landlock is disabled).
   - Trigger a learning run.
   - Expect: the bwrap fallback is attempted; bwrap spawn fails with the `setting up uid map: Permission denied` signature; the classifier latches `BwrapBrokenCause::AppArmorRestrictUserns`; subsequent streams see the latch and run unwrapped; the AppArmor-specific diagnostic (FR-015 template) appears in (a) the `tauri dev` terminal stderr (via `log::error!`) and (b) the run's `learning_runs.logs` column; the run completes with degraded per-call records (first call `sandbox: "bwrap"` + `failure_kind: "spawn"`; subsequent calls `sandbox: "none"` + `success: true`); the run-history UI shows the not-FS-confined disclosure marker.

3. **Forced Landlock-unavailable + bwrap not installed (synthetic generic-degradation):**
   - Temporarily mask Landlock probe AND `mv /usr/bin/bwrap /usr/bin/bwrap.disabled` (or chmod -x; reverse after the test).
   - Trigger a learning run.
   - Expect: at detection time the chain falls straight to `None`; the **generic** diagnostic (FR-014 template) appears in stderr + run logs; the run completes with all calls recording `sandbox: "none"`; the run-history UI shows the not-FS-confined marker.

## Outstanding from prior features (unchanged)

- Feature 006 T012 (frontend render assertion) stays deferred — no frontend test infra was added; that's a separate decision.
- Feature 005 quickstart V-acceptance runs (T023/T036/T049/T057/T065/T070) remain tracked under `specs/005-learning-system-hardening/` and are unaffected.
