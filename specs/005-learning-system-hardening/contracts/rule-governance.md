# Contract: Rule Governance (R-2/R-3/R-6 — C-2/C-5, FR-007..018)

Internal contract: lifecycle, tombstone gate, sole-writer, legacy migration.

## Lifecycle (persisted `learned_rules.lifecycle`)

States: `candidate | awaiting_review | active | rejected | suppressed |
tombstoned`. Distinct from derived `state` (quality label, unchanged).
Transitions and guards: see [data-model.md](../data-model.md) state machine.

- Extraction writes **only** `candidate` (DB). **No `fs::write` exists on any
  analysis path** — the `above_threshold` branch (`learning.rs:1497`,
  `1530-1549`) is deleted (FR-007, Q1=A).
- `candidate → awaiting_review` iff `eligible_for_review` true:
  `evidence_weighted_score ≥ min_eligibility` (default 0.6) AND
  `resolved_distinct_refs ≥ 3` AND `distinct_sources ≥ 1` AND
  `state != invalidated` AND `!tombstone_blocks(name)` (FR-014/016).

## `tombstone_blocks(conn, name) -> bool`

True iff a `rule_tombstones` row exists for `name` AND
`reactivated_at IS NULL`. Consulted at ALL five name-addressed write/activation
paths: `store_learned_rule`, `write_rule_files`, `promote_learned_rule`,
`reconcile_learned_rules` step 3a, step 3c. `delete_learned_rule` and reconcile
step 3b WRITE a tombstone (`tombstoned_by` = `human`/`reconcile_delete`).
`ON CONFLICT(name)` is suppression-sticky on `file_path`/`content`. Clearing a
tombstone = explicit authorized `reactivate_rule` IPC only (FR-010).

## Sole writer: approval

`promote_learned_rule` is the ONLY function that writes a global `.md`.
Contract:
- Precondition: `lifecycle == 'awaiting_review'` (else `Err`); `!tombstone_blocks`.
- Effect (one tx): write `sanitize_rule_content(redact(content))` to the
  scope dir (path-traversal canonicalization kept); set `file_path`,
  `lifecycle='active'`; append `rule_versions` (`change_kind='promote'`);
  populate `origin_run_id/model/at` + `rule_evidence_citations` snapshot.
- Re-derivation of a pending/active rule: `ON CONFLICT(name)` UPSERTs content,
  bumps a pending version, sets `pending_changed=1`; never a 2nd queue row,
  never silent overwrite of an active `.md` (FR-007 edge case).

**Forward reconciliation (feature 006 Follow-up B, 2026-05-18):** the
"`sets pending_changed=1`" wording stays reconciled away — there is no
`pending_changed` column (data-model.md "As-built reconciliation (a)"); the
pending marker is and remains the `current_version` bump. Feature 006 also
moves *when* that bump happens: `store_learned_rule`'s `ON CONFLICT` no
longer increments `current_version`; it returns a `pending_changed` signal
and `write_rule_files` advances `current_version` only AFTER the new
version's `rule_evidence_citations` snapshot is persisted, atomically in one
transaction (`persist_citations_and_advance_version`). So a re-derived queued
rule's `current_version` always resolves to a version that has its evidence
citations (a citation-write failure rolls back the bump, leaving the rule
review-eligible on its prior snapshot — FR-010/SC-006); the α/β + content
merge still always commits (merge-always). Still no schema change.

`rollback_rule(name, target_version)` (authorized IPC): append a new
`rule_versions` row (`change_kind='rollback'`, `rolled_back_from`), restore
`learned_rules.content/content_hash/current_version`, rewrite the `.md` in the
same tx, hash-touched so the watcher does not re-version it (FR-009).

## Reconcile cooperation

`reconcile_learned_rules`: step 3a/3c skip names where `tombstone_blocks` OR
`lifecycle ∈ {tombstoned, rejected}`; step 3b writes a `reconcile_delete`
tombstone. Reconcile-ingested new files enter as `candidate` (NOT active) —
they route into the review queue, never auto-activate.

## Legacy archive-then-wipe (Q3=C, FR-012) — one-time, in migration 25 chain

Runs inside the migration (before `rule_watcher::start` → no race),
idempotent (sentinel `legacy_rules_archived`):
1. Copy every on-disk learned `.md` (claude/codex/shared) to
   `<data_local>/legacy-rules-archive/<ISO8601>/` (`0444`, outside watched
   dirs) + `ARCHIVE_MANIFEST.json` (orig path, sha256, scope, mtime).
2. Delete the on-disk files; mark matching DB rows `lifecycle='tombstoned'`,
   `tombstoned_by='legacy_archive'`.
3. They return ONLY via the new gated pipeline. SC-012: 0 active rules lack
   provenance; 100% recoverable from the archive.

## Run status (FR-013)

`learning_runs.status ∈ {running, completed, degraded, failed}` enforced in
Rust at `update_learning_run` call sites. `degraded` = ≥1 stream failed AND ≥1
succeeded; per-stream success recorded in `inference_metadata`.
`degraded`/`failed` write nothing.

## IPC authorization (H-4, FR-011)

State-changing learning IPC (promote/delete/approve/reject/rollback/
reactivate/suppress/feedback) require an ephemeral per-process capability token
+ `learning`-window-label assertion (constant-time compare, mirrors HTTP
`check_auth`). Read commands stay open. HTTP `post_learned_rule` clamped to
`lifecycle='candidate'` only.

## Acceptance
SC-002 (0% global without human approval), SC-003 (100% provenance), SC-004
(100% rollback-able; 0 resurrection over ≥5 cycles), SC-012 (legacy archived,
0 provenance-less active).
