# Quickstart — Maintainer Verification (Feature 006)

How to verify both follow-ups after implementation. No production code is written until this plan is approved.

## Automated baseline (authoritative, post-join)

Run from repo root on the integrated branch:

```bash
cargo fmt --check
cargo clean -p quill && cargo clippy --all-targets -- -D warnings   # forced clean re-lint
cargo test --lib
npm run build                                                       # frontend tsc + build
lat check                                                           # wiki links + code refs
```

All must pass with **zero new warnings**. `cargo check` is NOT a substitute (it does not run clippy; cached clippy hides warnings — hence the forced `cargo clean -p quill`).

## Follow-up A — confinement honesty

Automated (deterministic, offline — no live `claude`, no network):
- `sandbox_metadata_is_recorded_for_every_call` passes with the extended closed set / Linux `platform_expected`.
- The new pure `SandboxKind` mapping test passes (for every variant `as_str()` ∈ closed set; `sandbox_tag_is_fs_confined(as_str())` matches the FS/non-FS table).
- The `RunHistory` render assertion: a not-FS-confined run shows the distinct marker + remediation hint; an FS-confined run does not.

Manual V-acceptance:
- On a host **with** `bwrap`: trigger a learning run → run history shows the FS-confined state (unchanged from feature 005).
- On a host **without** `bwrap` (e.g. `PATH` without it): trigger a learning run → it completes (never fail-closed), the persisted `sandbox` tag is the honest process-only tag (not an FS-confinement tag), and run history shows the distinct "no filesystem confinement — install bwrap" disclosure.

## Follow-up B — version/evidence atomicity

Automated (deterministic; `#[test] #[serial]` + `TempDir`):
- Eligibility-preserved-across-re-derivation: a v1 review-eligible pending rule re-derived with changed content stays eligible; `current_version` becomes v2 only after v2 citations exist.
- Citation-failure injection: forcing the citation step to fail leaves `current_version` un-advanced and the rule still review-eligible on its prior snapshot.
- Unchanged-content no-op: identical re-derivation → no version bump, no eligibility change.
- Regression: `store_learned_rule_on_conflict_is_suppression_sticky` and `eligible_for_review_enforces_min_cluster_uniformly_across_streams` pass unchanged.

Manual V-acceptance:
- Seed a pending rule that is surfaced for review; re-derive it (changed content) and confirm it stays in the review queue continuously (no flicker to `candidate`/not-eligible).

## Feature-005 manual runs (still tracked, unaffected)

The feature-005 quickstart V-acceptance runs (T023/T036/T049/T057/T065/T070) remain tracked under `specs/005-learning-system-hardening/` and are not modified by this feature.
