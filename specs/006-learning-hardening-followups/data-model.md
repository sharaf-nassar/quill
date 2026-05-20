# Phase 1 Data Model — Learning System Hardening Follow-ups

The recommended options introduce **no SQLite migration**. This document records the (zero) schema impact, the in-memory/serialized shape changes, and the reconciliation hooks back into feature-005 docs.

## Schema impact

**None.** No `CREATE TABLE`/`ALTER TABLE`/migration is added.

- **Follow-up A**: `learning_runs.inference_metadata` / `optimization_runs.inference_metadata` remain the same JSON TEXT columns from migration 25. The `InferenceCallMetadata.sandbox` string is unchanged in shape; only its **vocabulary** gains an honest member for the Linux `bwrap`-absent path. Legacy/pre-feature-005 decoded records (no `sandbox`) remain tolerantly decoded.
- **Follow-up B**: `learned_rules` and `rule_evidence_citations` are unchanged (migration 25). `current_version` remains the pending-change marker (no `pending_changed`/`pending_version` column added — honors FR-013 and feature-005 "As-built reconciliation (a)"). Only the **ordering** of the existing writes changes.

## Entity / serialized-shape deltas

- **`SandboxKind`** (`src-tauri/src/cc_client.rs`): closed enum gains one variant for the honest Linux process-namespace-only path. A single pure tag-keyed classifier `sandbox_tag_is_fs_confined(&str) -> bool` (as-built: a `pub(crate)` free fn, not a duplicate typed method) is the sole source of truth, used by the decode/projection path and the totality test. Stable `as_str()` tags remain a closed vocabulary; the SC-013 closed-set and per-platform-expected sets in `sandbox_metadata_is_recorded_for_every_call` are extended accordingly. FS-confined tags = {`bwrap`, `sandbox-exec`}; not FS-confined = {`process-only`, `job-object`, `none`}.
- **`InferenceCallMetadata.sandbox`** (`src-tauri/src/cc_client.rs`): same `Option<String>` field; doc updated to state the tag distinguishes filesystem-confined from process/flag-only. Backward/forward compatible (additive vocabulary).
- **Frontend run-inference types** (`src/types.ts`): `RunInferenceCall` and/or `RunInferenceSummary` gain an additive optional `confinement` descriptor (the recorded tag + a derived `fs_confined: boolean`) projected from the backend summary. Additive and optional — no breaking change to existing consumers.

## Review-eligibility invariant (Follow-up B)

New, enforced-by-construction invariant (no schema change needed to state it):

> For any rule, `learned_rules.current_version` MUST resolve to a `rule_version` for which `rule_evidence_citations` already holds that version's snapshot — OR `current_version` has not yet been advanced for the pending change. The advance and the new-version snapshot are effectively atomic with respect to any reader of review eligibility.

## Reconciliation hooks into feature-005 docs

Add dated lines (do not rewrite history; mirror the feature-005 self-reconciliation style):

- `specs/005-learning-system-hardening/data-model.md` "As-built reconciliation (a)": note that **feature 006** moves the pending-marker `current_version` bump out of `store_learned_rule`'s `ON CONFLICT` CASE to a post-citation-persist step in `write_rule_files`; `current_version` remains the marker; still no `pending_changed` column.
- `specs/005-learning-system-hardening/contracts/rule-governance.md` re-derivation note: same forward-reference; the stale "sets `pending_changed=1`" wording stays reconciled (no such column) and now also reflects the post-citation ordering.
- Feature-005 R-7.6 / data-model "Inference Call Metadata": note that feature 006 makes the Linux `bwrap`-absent tag honest (process-namespace only, no FS confinement) and surfaces it in the UI.
