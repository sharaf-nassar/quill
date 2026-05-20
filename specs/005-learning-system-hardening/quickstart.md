# Quickstart: Learning System Hardening — Maintainer Verification

**Feature**: `005-learning-system-hardening` | **Date**: 2026-05-17

Manual verification walkthrough mapping each Success Criterion to a concrete
check. Per project policy automated tests are authored only where the feature
requires them (FR-021); the rest is verified here. Run after
`/speckit-implement`.

## Setup

1. Build: `cargo build --manifest-path src-tauri/Cargo.toml` (or the Tauri dev
   build). Confirm migration applied: DB `MAX(version)=25`.
2. Confirm legacy archive ran once: `<data_local>/legacy-rules-archive/<ts>/`
   exists with `ARCHIVE_MANIFEST.json`; the live learned-rule dirs
   (`~/.claude/rules/learned/`, `~/.config/quill/learned-rules/{codex,shared}`)
   contain no provenance-less `.md`.

## V1 — Privacy (SC-001, US1)

- Generate activity containing seeded markers: an `sk-ant-…` key, a
  `postgres://u:p@h/db` URL, a raw 40-hex token, an email.
- Inspect `observations` at rest (DB): every marker is `‹redacted›`, none raw.
- Trigger an analysis run; capture each inference input (debug log of the
  prompt corpus for Stream A/B/C, synthesis, memory-optimizer): **0**
  unredacted markers. Structural frame (key name, URL host, `@domain`) intact.
- Hand-edit an active rule `.md` to inject a secret + a fenced block; confirm
  reconcile stores it redacted + de-fenced.

## V2 — No autonomous promotion (SC-002, US2)

- Force a high-confidence extraction (synthetic strong pattern). After the run:
  rule exists in DB at `lifecycle='awaiting_review'`, `file_path=''`; **no
  `.md`** written to any global dir. Periodic timer running does not change
  this over multiple cycles.
- Approve via the Learning UI (authorized) → `.md` now written,
  `lifecycle='active'`.

## V3 — Provenance & rollback (SC-003, SC-004, US2)

- For any active rule: UI shows origin run, model, timestamp, ≥1 evidence
  citation (resolves or shows retention-purged snapshot, not blank).
- Edit the rule, then roll back: prior content restored on disk + DB; a
  `rule_versions` row with `change_kind='rollback'` recorded.
- Delete a rule; run ≥5 analysis cycles re-deriving the same pattern: it does
  NOT reappear active (tombstone holds). `reactivate_rule` brings it back only
  on explicit action.

## V4 — Evidence-grounded eligibility (SC-005, SC-006, US3)

- Inject a candidate from a single source / <3 distinct citations: NOT
  surfaced as eligible regardless of stated confidence.
- Inject a candidate citing a non-existent observation id / commit / session:
  rejected pre-persist (log line `Rejected '<name>': unresolved evidence`).
- Confirm the eligibility decision uses `evidence_weighted_score` (toggle
  freshness/contradiction and watch eligibility change; raw model confidence
  alone never promotes). Conflicting pair → one `superseded`/both
  `conflict_flagged`, not both active. An `irrelevant` verdict measurably
  decays the rule's freshness/state.

## V5 — Evaluation & CI (SC-007, SC-008, US4)

- Run the eval harness on a candidate: returns a with/without verdict +
  regression/negative-transfer signal; result persisted linked to rule+run.
- Attempt to approve a rule whose latest verdict is `regression=1`: blocked
  until a `reviewer_overrides` row (with reason) is recorded.
- `cargo test` green locally; introduce a deliberate defect in
  `wilson_lower_bound`/`compute_state`/synthesis-decision → `cargo test` fails;
  open a PR → `ci.yml` red and blocks; revert → green.

## V6 — Observability & correctness (SC-009, SC-010, US5)

- RunHistory shows per-run cost, inference time, model actually used,
  observation count, and `completed|degraded|failed` status. Force one stream
  to fail → run shows `degraded` (not `completed`), surviving streams still
  produced candidates, nothing written to disk.
- Synthesis log/metadata names the pinned `claude-sonnet-4-6` (no rolling
  alias).
- With unanalyzed observations newer than the last successful run, trigger
  cleanup: those observations are NOT deleted (SC-010). Summary+delete is
  atomic (kill mid-cleanup → no rows deleted without the summary).
- Shared-scope rule UI discloses the Codex Bash-only capture limitation with
  quantified per-provider counts.

## V7 — Sandbox (FR-005, SC-013, US1)

- Inspect `InferenceCallMetadata` for a run: the `sandbox` field records the
  applied confinement (`bwrap`/`unshare`/`sandbox-exec`/`job-object`/`none`)
  for 100% of runs on every platform (SC-013).
- On a host with `bwrap`/`sandbox-exec`, confinement is active: an attempted
  out-of-workspace access from the spawned `claude` fails — it cannot read
  `~/.claude` or project trees outside the per-call temp dir; the model call
  still succeeds (network preserved).
- On a platform with no usable OS confinement (e.g. some Windows configs),
  learning is NOT disabled — the run proceeds flag-isolated and the reduced
  confinement is recorded/disclosed (FR-005 never-fail-closed).

## V8 — Learning value preserved (SC-011)

- Compare against `specs/005-learning-system-hardening/baseline.md` (captured
  by the Setup baseline task before any change): the count and reviewer-judged
  usefulness of genuinely useful discovered rules is ≥ the pre-remediation
  baseline (redaction/grounding/min-cluster did not gut real rules). Tune
  `min_eligibility`/`min_evidence_count`/entropy threshold if regressed.

## Notes

- All `[NEEDS CLARIFICATION]` resolved at spec stage (Q1=A/Q2=B/Q3=C).
- Migration is **25** (not 21 — plan corrected; assert post-migration).
- SC-011 has a captured baseline anchor (Setup baseline task → `baseline.md`);
  SC-013 covers FR-005 sandbox confinement (recorded every run).
- T-numbered execution + ordering constraints come from `/speckit-tasks`
  (research.md "Cross-cutting integration constraints" is the ordering source).
