# Phase 1 Data Model: Learning System Hardening

**Feature**: `005-learning-system-hardening` | **Date**: 2026-05-17
**Source**: [research.md](./research.md) (R-2/R-3/R-4/R-5/R-6 schema decisions)

All schema changes land in **one additive, transactional, idempotent
migration 25** in `src-tauri/src/storage.rs` (verified next free version;
21–24 already exist). Guards: `table_has_column` for `ALTER`,
`CREATE TABLE/INDEX IF NOT EXISTS`, single `conn.transaction()`,
`INSERT INTO schema_version (version) VALUES (25)`, post-assert `MAX=25`.

## Migration 25 — schema delta

### `learned_rules` (additive columns)

| Column | Type | Purpose | Finding |
|---|---|---|---|
| `lifecycle` | `TEXT NOT NULL DEFAULT 'candidate'` | Persisted lifecycle state (distinct from derived `state`) | R-3 |
| `origin_run_id` | `INTEGER` (→`learning_runs.id`) | Provenance: originating run | C-2 |
| `origin_model` | `TEXT` | Provenance: model snapshot at create/promote | C-2 |
| `origin_at` | `TEXT` | Provenance: RFC3339 origin timestamp | C-2 |
| `current_version` | `INTEGER NOT NULL DEFAULT 1` | Pointer into `rule_versions.version` | C-2 |
| `superseded_by` | `TEXT` | Survivor rule name when duplicate | M-3 |

`state` (existing) is **retained unchanged** as the read-time-derived quality
label (`emerging`/`confirmed`/`stale`/`invalidated`); it is NOT the lifecycle.
`confirmed_projects` (existing dead column) is **repurposed** to a JSON array of
distinct project paths among a rule's resolved citations (R-6 distinct-sources).

### New tables

**`rule_versions`** — append-only content history (C-2, FR-009)
`id` PK, `rule_id`→`learned_rules(id) ON DELETE CASCADE`, `version` (1-based,
`UNIQUE(rule_id,version)`), `content`, `content_hash`, `domain`,
`is_anti_pattern`, `provider_scope`, `source`, `run_id`, `change_kind`
(`create|update|manual_edit|promote|rollback`), `rolled_back_from`, `author`
(`system|human|reconcile`), `created_at`. Index `(rule_id, version DESC)`.

**`rule_evidence_citations`** — retention-proof grounding (C-2, H-1; shared by
R-2+R-6, defined once)
`id` PK, `rule_id`→CASCADE, `run_id`, `rule_version`, `observation_id`
(nullable soft ref — NO FK, `observations` is purged), `provider`,
`session_id`, `cwd`, `tool_name`, `evidence_ts`, `snippet` (redacted, bounded,
≤8/version), `kind` (`observation|commit|session`), `ref_id` (the cited
id/hash), `created_at`. Indexes `(rule_id,rule_version)`, `(run_id)`,
`(observation_id)`.

**`rule_tombstones`** — durable suppression (C-5, FR-010)
`rule_name` **PRIMARY KEY** (name = stable identity across re-extraction/
reconcile), `rule_id` (advisory), `tombstoned_at`, `tombstoned_by`
(`human|reconcile_delete|legacy_archive`), `reason`, `last_content_hash`,
`reactivated_at` (NULL ⇒ active block), `reactivated_by`. Index
`(reactivated_at)`. **Never CASCADE-deleted** — must outlive the rule row.

**`operator_feedback`** — primary outcome signal (Q2=B, FR-029)
`id` PK, `rule_name`, `actor` (default `'operator'`), `feedback`
(`accept|reject|bad`), `note` (nullable, maintainer-only, never sent to
inference), `rule_content_hash` (attribution across edits), `created_at`,
`updated_at`. `UNIQUE(rule_name, actor)` (revisable upsert).

**`evaluation_results`** — counterfactual verdicts (C-4, FR-022)
`id` PK, `rule_name`, `learning_run_id`→`learning_runs(id)`,
`replay_set_version`, `judge_model`, `evaluated_at`, `verdict`
(`helps|neutral|regresses|inconclusive`), `delta` REAL, `regression` INT,
`negative_transfer` INT, `judge_uncalibrated` INT, `replay_set_stale` INT,
`agreement_score` REAL, `rationale`, `per_case_json`. Index
`(rule_name, evaluated_at DESC)`.

**`reviewer_overrides`** — audited regression overrides (C-4, FR-020)
`id` PK, `rule_name`, `replay_set_version`, `overridden_by`, `reason`
(required), `overridden_at`. Becomes part of provenance.

`learning_runs.status` — no schema change; value set tightened in Rust to the
closed enum `running|completed|degraded|failed`.

## Entities (spec → model mapping)

| Spec entity | Realized as |
|---|---|
| Observation | `observations` (redacted at capture, R-1) |
| Learned Rule | `learned_rules` + `lifecycle` + provenance cols |
| Rule Version | `rule_versions` |
| Provenance Record | `learned_rules.origin_*` + `rule_evidence_citations` + `learning_runs.inference_metadata` |
| Evidence Citation | `rule_evidence_citations` |
| Promotion Decision / Approval | `rule_versions(change_kind='promote')` + `reviewer_overrides` + IPC actor |
| Suppression Tombstone | `rule_tombstones` |
| Evaluation Result | `evaluation_results` |
| Replay Set | in-repo `src-tauri/tests/fixtures/replay_set/` + `manifest.json` |
| Outcome Signal | `operator_feedback` |
| Analysis Run | `learning_runs` (status enum + decoded inference summary) |

## State machine — rule lifecycle (persisted `lifecycle`)

```
            extraction (NO fs::write — Q1=A)
                       │
                       ▼
                 ┌───────────┐  evidence-weighted gate fails / min-cluster unmet
                 │ candidate │──────────► stays candidate (DB only, not surfaced)
                 └───────────┘
                       │ score ≥ min_eligibility(0.6) AND ≥3 distinct resolved
                       │ citations AND state≠invalidated AND not tombstoned
                       ▼
              ┌────────────────┐  later run re-derives, different content
              │ awaiting_review│◄─── UPSERT same row, bump current_version
              └────────────────┘     (pending marker; never duplicate/overwrite)
                 │            │
   human approve │            │ human reject (durable)
   (authorized   │            ▼
    IPC; sole    │      ┌──────────┐
    .md writer)  │      │ rejected │
                 ▼      └──────────┘
            ┌────────┐  rollback → restore rule_versions snapshot, stays active
            │ active │  (new version row, .md rewritten in same tx)
            └────────┘
              │   │
   suppress/  │   │  duplicate → superseded(+superseded_by)
   delete or  │   │  conflict  → conflict_flagged (both; human-resolved)
   feedback=  │   ▼
   bad        │ ┌────────────┐
              └▶│ tombstoned │ durable; reconcile/extract MUST NOT resurrect;
                └────────────┘ reactivation = explicit authorized IPC only
```

**As-built reconciliation (feature 005, 2026-05-18 — doc matches the build; no code change):**

- **(a) No `pending_changed` column.** Migration 25 adds no such column; the
  earlier diagram label was wrong. The pending-change marker for an
  `awaiting_review` rule that is re-derived with different content is a
  `current_version` bump on the same `learned_rules` row (`store_learned_rule`
  `ON CONFLICT(name)` increments `current_version` only when
  `lifecycle='awaiting_review'` AND the new content differs); the row is never
  duplicated and an `active` rule's on-disk `.md` is never overwritten by
  re-extraction (only `promote_*` writes disk).
- **(b) `rule_versions` is appended on promote/rollback only**, NOT on every
  re-derivation. `promote_learned_rule` appends a `change_kind='promote'` row
  and `rollback_rule` appends a forward `change_kind='rollback'` restore row;
  a queued-rule re-derivation only bumps `current_version` (see (a)) and does
  not append a version row.
- **(c) Provenance (`origin_run_id`/`origin_model`/`origin_at`) is
  best-effort.** It is captured at promote from the most recent
  `completed`/`degraded` `learning_runs` row (model snapshot decoded from that
  run's `inference_metadata`); `origin_model` may be `None` when no metadata is
  present. This mirrors the US3 evidence-citation behavior (citations resolved
  from the latest completed/degraded run until an evaluated/approved rule).
- **(d) `evaluation_results.replay_set_version` is stored as `TEXT`** (the
  numeric `replay_set_version` is `.to_string()`-serialized on persist), and
  `per_case_json` holds the serialized scalar `EvalVerdictRow` itself (T052
  as-built — the row is self-describing even though `EvalVerdictRow` carries no
  per-case detail).
- **(e) Lifecycle terminal states.** The persisted `lifecycle` set includes
  `superseded` (duplicate; `superseded_by` set) and `conflict_flagged` (both
  members of a conflicting pair; human-resolved), introduced by US3. They are
  shown as transitions in the diagram above and are valid terminal/quarantine
  states alongside `rejected`/`tombstoned`/`suppressed`.

**Forward reconciliation (feature 006 Follow-up B, 2026-05-18 — supersedes
the *timing* described in (a); the schema and marker are unchanged):**

- The pending-change marker is STILL the `current_version` bump on the same
  `learned_rules` row, and migration 25 STILL adds no `pending_changed`
  column (data-model constraint (a) and FR-013 are preserved). What changes
  is *where/when* the bump happens: `store_learned_rule`'s `ON CONFLICT(name)`
  CASE no longer increments `current_version`. It now only *detects* the
  pending content change (same trigger condition: `lifecycle='awaiting_review'`
  AND old/new content both non-null AND differ) and returns a `pending_changed`
  signal. `write_rule_files` persists the new version's
  `rule_evidence_citations` and advances `current_version` to that version in
  ONE transaction (`persist_citations_and_advance_version`), AFTER the merge.
  This makes "`current_version` always resolves to a version that has its
  evidence citations" hold by construction (closes the transient
  concurrent-reader window and the persistent case where a non-blocking
  citation-write failure left a human-pending rule permanently un-reviewable);
  the α/β + content merge still always commits (merge-always preserved). The
  row is still never duplicated and an `active` rule's `.md` is still never
  overwritten by re-extraction.

Guard `tombstone_blocks(name)` (row exists AND `reactivated_at IS NULL`) is
checked at all five name-addressed write paths: `store_learned_rule`,
`write_rule_files`, `promote_learned_rule`, `reconcile` steps 3a/3c.
`ON CONFLICT(name)` is suppression-sticky on `file_path`/`content` (evidence
still accrues to alpha/beta; re-arming gated).

## State machine — analysis run status

```
running ──▶ completed   (all dispatched streams ok; candidates may be written)
        ├─▶ degraded    (≥1 stream failed AND ≥1 succeeded; candidates only
        │                 from surviving streams; visibly disclosed per-stream)
        └─▶ failed       (0 usable findings OR synthesis/apply hard-error)
```
Invariant: `degraded`/`failed` write **nothing** to disk (trivially true — no
run writes disk anymore; `failed` writes zero candidates).

## Evidence-weighted scoring (single source of truth)

`evidence_weighted_score(alpha, beta, last_evidence_at) -> (score, state)`:
`fresh = freshness_factor(last_evidence_at)` (90-day half-life);
`score = wilson_lower_bound(alpha*fresh, beta*fresh)`; `state` via revised
`compute_state` (adds `beta>=alpha && beta>=5.0 → invalidated`). Called by both
`get_learned_rules` read sites AND `eligible_for_review` — read and gate can
never diverge. Operator feedback weight `W_op ≫` any single LLM verdict
strength: `accept`→large α; `reject`→large β (no tombstone); `bad`→largest β +
tombstone. Raw LLM `rule.confidence` no longer gates anything.

## Validation rules (from requirements)

- FR-001/003: every `observations.tool_input/output/cwd` and every inference
  input redacted before persistence/compression; mask token idempotent.
- FR-008: `origin_run_id/model/at` non-null at promote; ≥1 resolvable citation.
- FR-015: candidate with zero resolvable `evidence_refs` → rejected pre-persist.
- FR-016: `resolved_distinct_refs ≥ 3 AND distinct_sources ≥ 1` for review
  eligibility, uniform across streams; per-rule `observation_count` = resolved
  citation count (fixes B/C `=0`).
- FR-009: `rule_versions` append-only; rollback is a forward restore row.
- FR-010: tombstone active ⇒ no path re-activates without explicit reactivation.
- FR-013: `status ∈ {running,completed,degraded,failed}` enforced in Rust.
- FR-020: approval of `regression=1` blocked unless a `reviewer_overrides` row
  exists for `(rule_name, replay_set_version)`.
- FR-026: no observation deleted with `created_at >` last
  completed/degraded run start; summarize+delete atomic.
