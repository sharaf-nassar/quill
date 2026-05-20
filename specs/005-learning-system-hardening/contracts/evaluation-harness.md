# Contract: Evaluation Harness + Tests + CI (R-4 / C-4, FR-019..023, FR-021)

## Replay set (frozen, in-repo)

`src-tauri/tests/fixtures/replay_set/`:
- `case_NNN_<slug>.json`: `{ inputs:{corpus,existing_rules_summary,
  provider_scope,stream}, rule_under_test:{name,content,domain,
  claimed_confidence}, expected_judgment:{verdict:helps|neutral|regresses,
  rationale}, tags[] }` — pre-redacted (R-1), maintainer-authored labels.
- `manifest.json`: `{ replay_set_version:int, baseline_assistant_model:str,
  frozen_at:rfc3339, schema_version:int, cases:[ids] }`.
- Coverage tags span: positive, regressing/negative-transfer, hallucinated-
  evidence, one-off/low-evidence, conflicting-pair, suppressed-rederived, empty.
- Staleness (FR-023): stale iff judge model ≠ `baseline_assistant_model` OR
  `age(frozen_at) > REPLAY_STALENESS_DAYS` (≈90). Disclosed, not auto-blocking.

## `src-tauri/src/eval_harness.rs`

Per case: WITH/WITHOUT paired `cc_client::invoke_typed` (identical inputs ±
`rule_under_test`), pinned `Model::Sonnet46`, N=3, majority/median; then a
judge call → typed:
```rust
struct EvalVerdict { with_quality:f64, without_quality:f64, delta:f64,
  regression:bool, negative_transfer:bool, rationale:String }
```
`regression = delta < -EPSILON (≈0.05) || negative_transfer`. Calibration mode
(also CI): score judge vs frozen labels → agreement κ; `< ~0.6` ⇒
`judge_uncalibrated=true` (verdicts advisory, do not block). High per-arm
variance ⇒ `inconclusive`.

## Persistence (FR-022)
One `evaluation_results` row per `(rule_name, learning_run_id,
replay_set_version)` incl. `per_case_json` (schema = data-model.md; DDL owned
by migration 25 / R-2).

## Promotion coupling (FR-020) — interface to governance
```
latest_eval_verdict(rule_name, replay_set_version) -> Option<EvalVerdictRow>
has_reviewer_override(rule_name, replay_set_version) -> bool
```
`approve` MUST deny if `latest.regression && !has_override`; override = audited
`reviewer_overrides` row (required reason). `inconclusive|uncalibrated|stale`
→ warn, not block. No eval yet → surfaced "unevaluated" (SC-007).

## Unit tests (FR-021) — seams
`wilson_lower_bound`, `compute_state` (incl. new β-override), `freshness_factor`,
`evidence_weighted_score`/`eligible_for_review`, synthesis-decision matrix
(`learning.rs:1183-1275`, incl. insights-only succeeds), suppression durability
(tombstone survives re-extraction), grounding rejection, min-cluster,
eval pure logic (dead-band, majority-of-N, κ, staleness). Style: in-crate
`#[test]`/`#[tokio::test]`, `TempDir`+`#[serial]` (existing pattern). **Inference
double**: narrow injectable trait / `#[cfg(test)]` `OnceLock` hook in
`cc_client.rs` returning scripted `StreamFindings`/`EvalVerdict`/
`InferenceError` — production free-fn signatures unchanged; no live `claude`.

## CI gate (FR-021)
New `.github/workflows/ci.yml` on `pull_request` + `push:main`:
`cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test`
(offline via the double). Wired as `workflow_call` precondition of
`release.yml` → failing learning-logic suite blocks merge AND release.

## Acceptance
SC-007 (100% candidates get with/without verdict + regression signal; 100%
approved have non-regressing verdict or recorded override), SC-008 (known
defect in scoring/state/promotion fails CI 100%).
