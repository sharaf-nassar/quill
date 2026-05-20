# Contract: IPC + Operator Feedback (R-3/R-5/R-7 — H-4, FR-029, FR-024)

## Authorization model (H-4 / FR-011)

Ephemeral per-process capability token (generated at startup, `OsRng`),
injected only into the `learning` window via a label-gated
`get_learning_capability()` command. Every state-changing learning IPC takes
the token, verifies it constant-time, AND asserts the calling
`WebviewWindow::label()` is in the allowlist (`["learning"]`). Read-only
commands unauthenticated. HTTP `post_learned_rule` stays bearer-authed and is
clamped to `lifecycle='candidate'`.

## New / hardened Tauri commands

All return `Result<(), String>` and `app.emit("learning-updated", ())` on
success (existing convention); `useLearningData` already calls `refresh()` on
that event. Names validated via `is_safe_rule_name`.

| Command | Args | Effect |
|---|---|---|
| `submit_rule_feedback` | `name, feedback:"accept"\|"reject"\|"bad", note?` | Upsert `operator_feedback` (key `name,actor`); `accept`→large α; `reject`→large β; `bad`→largest β + write tombstone (authorized). |
| `approve_rule` | `name, token` | Governance sole-writer (see rule-governance.md); denies if regressing w/o override. |
| `reject_rule` | `name, token` | `lifecycle='rejected'` (durable; not auto-requeued). |
| `rollback_rule` | `name, target_version, token` | Forward-restore version + rewrite `.md` (one tx). |
| `reactivate_rule` | `name, token` | Clear tombstone (`reactivated_at/by`); only path that un-blocks. |
| `record_reviewer_override` | `name, replay_set_version, reason, token` | Audited `reviewer_overrides` row enabling approval of a regressing rule. |
| `promote_learned_rule`*, `delete_learned_rule`* | (existing) + `token` | Now authorized; promote gated on `awaiting_review`. |

`feedback="bad"` changes active state → requires the token path; `accept`/
`reject` need only the same trust level as today's promote/delete.

## Feedback model (Q2=B / FR-029)

`OperatorFeedback{Accept,Reject,Bad}`; `bad` distinct from `reject` (reject =
down-weight, recoverable; bad = strongest negative + durable tombstone). One
per `(rule_name, actor)`, revisable; stores `rule_content_hash` at feedback
time. `note` is maintainer-only local metadata — **never** sent to any
inference input. Operator feedback weight `W_op ≫` any LLM verdict / raw
self-rating (human dominates). Rule-level judgment only — no end-user
satisfaction telemetry (consistent with feature 004 exclusions).

## UI (R-5)

Extend `src/components/learning/RuleCard.tsx` (no new component): three header
actions next to promote/delete. `accept`/`reject` = optimistic single click +
toast; `bad` = existing two-step inline confirm (matches promote pattern).
Current feedback shown as a meta badge. Threaded via
`useLearningData.submitRuleFeedback` (mirrors `deleteRule`/`promoteRule`).
Reused in the R-3 review-queue surface.

## Run history surfacing (R-7 / FR-024)

`LearningRun` gains `inference: Option<RunInferenceSummary>` (derived rollup:
`total_cost_usd`, `total_duration_ms`, `primary_model`, `call_count`,
`failed_call_count`, per-phase `calls[]`) decoded tolerantly from existing
`learning_runs.inference_metadata` (NULL/parse-error ⇒ `None`). No migration,
no new endpoint (serde auto-propagates через `get_learning_runs` IPC + HTTP
mirror). `RunHistory.tsx` adds Model / Cost / Inference-time rows + a
`degraded` status icon (amber) + a derived "N consecutive failed runs" banner
(presentational only, no circuit-breaker). `observations_analyzed` already
rendered.

## Acceptance
SC-009 (accurate per-run cost/latency/model/status for 100% of runs);
SC-002/004 (authorized, reversible mutations).
