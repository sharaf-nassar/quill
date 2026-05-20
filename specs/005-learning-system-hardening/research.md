# Phase 0 Research: Learning System Hardening

**Feature**: `005-learning-system-hardening` | **Date**: 2026-05-17
**Input**: [spec.md](./spec.md), [plan.md](./plan.md), behavioral-learning audit roadmap

Seven parallel research agents grounded each design domain in the actual code.
Format: **Decision / Rationale / Alternatives considered**. All decisions are
committed (no NEEDS CLARIFICATION); spec clarifications Q1/Q2/Q3 were resolved
before planning.

## R-0 — Critical cross-cutting correction (migration number)

**Decision**: The schema migration this feature adds is **migration 25**, NOT 21.

**Rationale**: Independently verified by 4 agents — `storage.rs:1761` records
through **migration 24** (`skill_usages` = 21, `skill_usages.cwd/hostname` = 22,
reingest re-arm = 23, `inference_metadata` = 24). The plan template's "next is
21" was stale. Committing as `21` would **silently no-op** (`current_version
(24) < 21` is false) and `INSERT INTO schema_version (version) VALUES (21)`
would collide with the existing PK. plan.md is corrected. data-model.md and all
contracts use **25**. Post-migration assertion `MAX(version)=25` (mirrors the
existing test at `storage.rs:6506-6513`).

**Alternatives**: none — this is a factual correction.

## R-1 — Redaction-at-capture & universal sanitization (C-1, H-3)

**Decision**: New module `src-tauri/src/redaction.rs` exposing
`pub fn redact(&str) -> String`. Layered detector: existing anchored
cred-prefix patterns (kept, broadened with `compress_prose/detect.rs`
`SENSITIVE_NAME_TOKENS`) + connection-string/URL userinfo + Shannon-entropy
fallback (≥4.0 bits/char, charset+length+git-hash/path gated) + email
local-part. Single idempotent mask token `‹redacted›`. Redact in
`post_observation` (`server.rs:540`) **before** `store_observation_in_background`
(keeps the `202` fast-ack synchronous immediately after) + defense-in-depth in
`store_observation` before INSERT → **no plaintext secrets at rest**. Universal
pipeline order everywhere: `redact → compress/truncate/sanitize`; invert the
wrong Stream-C order at `learning.rs:142`. Add explicit `redact` calls
(redact-first, before cache) to Stream B (`git_analysis.rs`, before
`compress_git_data` so `git_snapshots.raw_data` caches redacted), synthesis
contexts (`learning.rs:997-1028`), and `memory_optimizer::build_prompt`. Stream
A needs no change (capture guarantees it). H-3: route reconcile steps 3a/3c and
the promote path through `redact` → `sanitize_rule_content`; fix
`sanitize_rule_content` (`learning.rs:1567-1581`) to actually strip code fences
per its doc-comment. One-time backfill of existing `observations` +
`git_snapshots.raw_data`.

**Rationale**: `redact_secrets` has exactly one call site today
(`learning.rs:142`); every other inference input leaks. Capture-side redaction
makes Stream A automatically safe and is the only way to satisfy "no secrets at
rest". Redact-before-compress is security-correct (truncation-first can split a
secret so the anchored regex misses it). A named module makes "what is a
secret" one auditable definition.

**Alternatives**: redact at read-time only (rejected — leaves plaintext at
rest, burdens every consumer = the current bug); keep in `prompt_utils.rs`
(rejected — conflates injection-defense with secret-redaction); name/NER PII
(rejected — no reliable offline Rust lib, catastrophic false positives shred
behavioral signal; email-only is the committed PII scope).

**Risks**: entropy false positives (tunable 4.0-bit knob — biggest quality
lever, validate against real samples); capture-path latency vs fast-ack
(bounded by `MAX_TOOL_DATA_LEN`; fallback = move into the `spawn_blocking`
body, still pre-INSERT); already-captured plaintext (backfill recommended).

## R-2 — Provenance + version history + durable tombstone (C-2, C-5)

**Decision**: **Migration 25**, transactional + idempotent (`table_has_column`
guards, `CREATE TABLE/INDEX IF NOT EXISTS`). Add to `learned_rules`:
`origin_run_id`, `origin_model`, `origin_at`, `current_version`. New tables:
`rule_versions` (append-only content snapshots, `UNIQUE(rule_id,version)`,
`change_kind` incl. `rollback` + `rolled_back_from`); `rule_evidence_citations`
(denormalized redacted `snippet` + soft nullable `observation_id`, retention-
proof); `rule_tombstones` (**name-keyed**, active iff row exists AND
`reactivated_at IS NULL`). C-5 fix = three parts: (i) name-keyed tombstone
written by `delete_learned_rule` + reconcile step 3b; (ii) one
`tombstone_blocks(name)` gate consulted at all five name-addressed
write/activation paths (`store_learned_rule`, `write_rule_files`,
`promote_learned_rule`, `reconcile` 3a/3c); (iii) suppression-sticky
`ON CONFLICT` on `file_path`/`content` (evidence still accrues; re-arming is
gated). Reactivation is an explicit authorized IPC. Provenance survives
observation purge via the denormalized citation snapshot (captured post-R-1
redaction). Rollback = forward append-only restore (DB row + on-disk `.md` in
one tx, hash-touched so the watcher doesn't re-version it).

**Rationale**: `learned_rules.state` is **recomputed every read** by
`compute_state` and only ever consulted as `!= 'suppressed'` — it is not
authoritative, so a durable tombstone **must** be an independent gate table,
not a state value. Columns-on-`learned_rules` keep the hot read path
single-table; separate append-only tables keep history auditable and let the
tombstone outlive any row.

**Alternatives**: tombstone as `state='tombstoned'` (rejected — clobbered every
read; the core C-5 trap); JSON history blob (rejected — unbounded, not
indexable); id-keyed tombstone (rejected — re-extraction/reconcile are
name-addressed); SQLite triggers (rejected — can't emit the UI log breadcrumb,
poor auditability).

**Risks**: `ON CONFLICT` hardening changes evidence-vs-armed semantics
(intended per FR-010, needs reviewer sign-off); citation snippet storage growth
(bound length, ≤8 citations/version); reconcile/approval race (closed by
tombstone checks in reconcile 3a/3c).

## R-3 — Review queue, autonomous-write removal, legacy wipe, run status (FR-007/012/013, H-4)

**Decision**: New persisted **`lifecycle`** column on `learned_rules` (distinct
from the derived quality label `state`): `candidate → awaiting_review →
active/rejected`, plus `suppressed`/`tombstoned`. **Delete the `above_threshold`
auto-write branch entirely** (`learning.rs:1497`,`1530-1549`) — analysis only
ever writes DB candidates; periodic timer unchanged (now harmless); `micro` was
already dead. **Approval = sole writer**: harden `promote_learned_rule` into
the only global-`.md` writer, gated on `lifecycle='awaiting_review'`, recording
`rule_versions` + provenance; re-derivation UPSERTs (idempotent) with a
`pending_changed` flag — never duplicates/silently overwrites a pending item.
**IPC auth (H-4)**: ephemeral per-process capability token + `learning`-window-
label assertion on all state-changing learning commands
(promote/delete/approve/reject/rollback/suppress/feedback); reads stay open;
HTTP ingest clamped to `candidate` only. **Legacy wipe (Q3=C)**: a one-time
step inside the migration chain (runs before the watcher starts → no race) that
copies all on-disk learned `.md` to a read-only `0444` manifested archive
**outside watched dirs**, deletes them from active use, tombstones their DB
rows; reconcile made tombstone/rejected-aware (skip re-INSERT). **Run status
(FR-013)**: closed tri-state `completed`/`degraded`/`failed` (+`running`),
enforced in Rust; degraded/failed write nothing (free — disk writes are gone).

**Rationale**: FR-007/SC-002 require zero extraction→global-`.md` paths;
deleting (not flag-gating) the branch is the only way to satisfy "no autonomous
mode exists". A separate `lifecycle` column is mandatory because `state` is
clobbered every read. Running the wipe in the migration chain eliminates the
watcher race by construction (init order already guarantees migrations precede
`rule_watcher::start`).

**Alternatives**: flag-gate auto-write (rejected — that *is* the autonomous
mode Q1=A removes); reuse `state` for lifecycle (rejected — recomputed every
read); soft-suppress legacy (rejected — not durable = C-5); lazy wipe at
startup (rejected — racier than the migration chain).

**Risks**: reconcile re-INSERT vs tombstone must ship together with the
lifecycle column; `min_confidence` setting becomes inert (R-6 redefines it);
pending-candidate overwrite under re-derivation must special-case `ON CONFLICT`.

## R-4 — Evaluation/replay harness + tests + CI gate (C-4, FR-019..023/021)

**Decision**: New `src-tauri/src/eval_harness.rs`. Replay set = version-
controlled in-repo `src-tauri/tests/fixtures/replay_set/` of frozen,
pre-redacted judgment cases (digest corpus + existing-rules summary + rule-
under-test + maintainer-authored `expected_judgment` label) + `manifest.json`
(pinned baseline model, `frozen_at`, `replay_set_version`). Counterfactual =
per-case **WITH/WITHOUT** paired `cc_client` calls + a calibrated judge call
emitting typed `EvalVerdict{with_quality,without_quality,delta,regression,
negative_transfer,rationale}`; regression = signed `delta` past a dead-band OR
negative-transfer. **Judge calibrated** against frozen labels (agreement floor
≈κ0.6); majority-of-N=3 dampens non-determinism; uncalibrated/stale →
advisory, not blocking. `evaluation_results` persisted linked to
(rule, run, replay_set_version) — **DDL owned by R-2's migration 25**.
Promotion coupling: approval handler MUST consult latest verdict; regressing →
**blocked unless an audited explicit reviewer-override record** exists;
uncalibrated/stale → warn-not-block. Tests: `#[test]`/`#[tokio::test]` for
`wilson_lower_bound`, `compute_state`, `freshness_factor`, evidence-weighted
gate, synthesis-decision matrix (`learning.rs:1183-1275`), suppression
durability — reuse the existing `TempDir`+`#[serial]` Storage pattern; a narrow
injected inference double in `cc_client.rs` makes synthesis/eval paths testable
offline (no live `claude`). New `.github/workflows/ci.yml` (PR + push-to-main):
`cargo fmt --check` + `cargo clippy -D warnings` + `cargo test`, wired as a
`workflow_call` precondition of `release.yml` (failing learning-logic suite
blocks merge AND release).

**Rationale**: System has no end-user outcome signal (Q2=B operator feedback is
the only one), so a "replay set" must be a frozen *judgment* corpus anchored to
maintainer labels; freezing in-repo makes it deterministic + CI-runnable
without a populated DB. Calibration operationalizes the audit guardrail
"LLM-as-judge ≠ ground truth".

**Alternatives**: snapshot live `usage.db` (rejected — non-deterministic, leaks
PII); single uncalibrated judge call (rejected — the audit anti-pattern);
PATH-shim fake `claude` (kept only as optional `cc_client` smoke test); tag-only
CI (rejected — regressions hit main undetected).

**Risks**: small labeled set → noisy κ (size ≥12 cases, treat κ as tripwire);
hard dependency on R-2 landing `evaluation_results` + override record in
migration 25 (ordering constraint for tasks.md).

## R-5 — Operator accept/reject feedback (Q2=B, FR-029)

**Decision**: Three-valued `OperatorFeedback{Accept,Reject,Bad}` — `bad` is
distinct from `reject` (reject = down-weight, stays recoverable; bad =
strongest negative + triggers durable suppression/tombstone via R-2/C-5).
Entity: one feedback per (rule_name, actor), revisable upsert, stores
`rule_content_hash` at feedback time (attribution across content changes);
physical table owned by R-2 (migration 25). UI: extend **`RuleCard`** (no new
component) with three header actions reusing the existing two-step inline
confirm for `bad`, optimistic single-click for accept/reject; thread via
`useLearningData.submitRuleFeedback` → existing `learning-updated`/`refresh()`.
IPC: one `submit_rule_feedback(name,feedback,note?)` mirroring
`promote_learned_rule` (validate `is_safe_rule_name`, emit `learning-updated`),
routed through R-3's state-change authz for `bad`. Evidence weighting: operator
feedback is the **primary** signal layered on the existing α/β+Wilson substrate
with weight `W_op` **dominating any LLM verdict and the raw self-rating**;
accept→strong α, reject→strong β (no tombstone), bad→strongest β + durable
tombstone. LLM verdict path retained as the weaker secondary signal for
un-reviewed rules. Rule-level maintainer judgment only — no end-user
satisfaction telemetry (consistent with feature 004 exclusions); the optional
free-text `note` is local maintainer-only metadata, never fed to inference.

**Rationale**: Reuses the proven α/β substrate (no parallel scoring); "human >
model" expressed as weight dominance + optional hard eligibility gate; aligns
`bad` with the existing suppression mechanism so C-5 has one tombstone path.

**Alternatives**: two-state accept/reject (rejected — loses the deliberate-
removal intent the spec calls out separately); numeric rating (rejected —
invites satisfaction-telemetry semantics the spec excludes); replace α/β with
feedback-only score (rejected — discards freshness/Wilson/un-reviewed signal).

**Risks**: depends on R-2 tombstone + R-3 authz + R-6 scoring constants
(declared dependencies; feedback persists first, weighting consumes it — safe
direction).

## R-6 — Evidence-weighted gate + grounding + cluster + verdicts/conflict (C-3,H-1,H-2,M-3,M-4)

**Decision**: One pure `evidence_weighted_score(alpha,beta,last_evidence_at)
-> (score,state)` in `storage.rs`; both `get_learned_rules` read sites + a new
`Storage::eligible_for_review()` call it (single source of truth). The gate
moves to `write_rule_files` **after** `store_learned_rule` (post-merge
state), sets `lifecycle=awaiting_review` vs `candidate` — no `fs::write`
(Q1=A). Single indexed point-read per candidate (≤3/run, no N+1). Replace
`learning.min_confidence` (0.95 raw) with `learning.min_eligibility` on the
Wilson scale, default **0.6** (= existing `confirmed` cutpoint). **Grounding
(H-1)**: add `evidence_refs: Vec<EvidenceRef{kind,id}>` to `StreamPattern`/
`AnalysisRule` (schemars auto-propagates into the typed schema); per-stream
namespaces — Stream A injects real observation `id` into the prompt; Stream B
adds `%h` short-hash (fallback: snapshot HEAD key); Stream C uses existing
`session_id`. Resolve in `write_rule_files` before persistence; zero-resolvable
→ reject+log+continue. **Min cluster (H-2)**: `resolved_distinct_refs >= 3 AND
distinct_sources >= 1`, uniform A/B/C, inside `eligible_for_review`; fix
`observation_count=0` by threading per-rule resolved citation count into
`store_learned_rule` instead of Stream A's shared `obs_count`. **Verdicts
(M-4)**: add `irrelevant` → monotone `decay_rule_freshness` (one 90-day
half-life backward, clamped); unknown verdict → logged not dropped;
`compute_state` uses alpha/beta (`beta>=alpha && beta>=5.0` → `invalidated`
override); gate excludes `invalidated`. **Conflict/dedup (M-3)**: deterministic
flag-and-supersede (survivor = higher evidence-weighted score, deterministic
tie-breaks) recorded via `record_rule_reconciliation` — duplicates →
`superseded`+`superseded_by`; conflicts → `conflict_flagged` (both, human-
resolved). Repurpose dead `confirmed_projects` as the cross-project
distinct-sources signal (fallback: explicit drop in migration 25).

**Rationale**: The Wilson+freshness math is already correct; the only defect is
*where* it's consulted. Real DB ids/SHAs/session-ids in the prompt make
fabrication detectable. Namespaced `kind` is mandatory because the 3 streams
cite different evidence substrates. Deterministic reconciliation is
testable/auditable (FR-021) where an LLM merge is not.

**Alternatives**: recompute Wilson from `rule.confidence` only (rejected — raw
self-rating renamed); single `observation_ids` field (rejected — B/C have no
obs rows); LLM-merge dupes (rejected — non-deterministic, untestable,
single-stream skips synthesis); map `irrelevant`→`contradict` (rejected —
conflates "N/A here" with "wrong").

**Risks**: shared migration 25 + shared `rule_evidence_citations` table with
R-2 (define once); state-string proliferation (centralize "is human-active?"
predicate); defaults (0.6/3) are tunable, validate against SC-011.

## R-7 — Observability + correctness polish + CLI sandbox (H-5/6/7, M-1/2/6, L-1/2/3)

**Decision**: **H-6** decode the existing `learning_runs.inference_metadata`
JSON (no migration) into a derived `RunInferenceSummary` rollup
(cost/latency/primary-model/call-count) on `LearningRun`; surface per-run
cost/model/inference-time + the new `degraded` status in `RunHistory.tsx`
(plumbing already end-to-end; tolerant decode → `None` for legacy/micro).
**H-7** point synthesis at the existing pinned `Model::Sonnet46`
(`learning.rs:826`) — no new constant; pipeline becomes single-model. **M-2**
retention cutoff = analyzed watermark (`MAX(created_at) FROM learning_runs
WHERE status IN ('completed','degraded')`), never delete observations newer
than that; safety floor only *adds* retention; zero successful runs ⇒ delete
nothing; wrap summarize+delete in one transaction (roll back if summary fails).
**M-1** **consume** `observation_summaries` (wire into the analytics trend tail
as the post-retention history) + tighten the naive `LIKE '%error%'` tally since
it's now read. **M-6** **explicitly disclose** (Codex Bash-only is a Codex
platform limit, not equalizable) — quantified per-provider contribution at the
shared-rule UI + scoped counts; no capture-pipeline change. **H-5** per-platform
OS confinement wrapper around the spawned `claude`: Linux `bwrap`→`unshare`
fallback (FS/IPC/PID namespace, **network preserved** for the model call),
macOS `sandbox-exec` deny-by-default profile, Windows Job Object + best-effort
restricted token (documented best-effort); RW carve-out = exactly the per-call
temp dir; **recorded confinement state** on `InferenceCallMetadata` (graceful
degradation, never fail-closed). **L-1** correct doc drift (lat.md synthesis-
model after H-7; annotate 004 spec "as-built superseded Haiku"). **L-2** add
the missing multi-model cost-tiebreak regression test. **L-3** surface a
derived consecutive-failure banner + first-class `degraded` status — no
circuit-breaker (out of scope; "unrecorded" = observability gap).

**Rationale**: The metadata is write-only today; a derived rollup matches how
`phases` is already surfaced with zero migration/endpoint surface. The rolling
`sonnet` alias silently drifts behavior AND cost attribution. The watermark
makes "had the opportunity to be analyzed" literal (FR-026/SC-010). Sandbox
flag-isolation is in-process to the agent; only an OS boundary is a real
control, but network must stay (the CLI makes the model call itself).

**Alternatives**: new metadata endpoint/columns (rejected — duplicates the
authoritative JSON, needs migration); remove `observation_summaries` (rejected —
destroys the only post-retention trend record); equalize provider capture
(rejected — Codex runtime doesn't emit non-Bash hooks; leveling Claude down
degrades the corpus); containerize `claude` / hand-rolled namespaces as primary
(rejected — heavy/error-prone; `bwrap`/`sandbox-exec` are vetted); fail-closed
sandbox (rejected — bricks the loop where `bwrap` absent; recorded-degradation
makes the gap auditable).

**Risks**: sandbox is the dominant cross-platform risk and asymmetric by
necessity (Windows best-effort, disclosed); network-preserved scoping is
deliberate (threat = FS exfil, not egress); R-7 itself adds no migration.

**2026-05-19 (feature 007 reconciliation)**: Landlock LSM is promoted to the
**primary** Linux inference confinement mechanism; bwrap is **kept as
fallback** (now position 2 in the chain `Landlock → Bwrap → None`). The R-7
"best-available OS-level confinement, never fail-closed, recorded confinement
state" framework is preserved — only the top tier of the hierarchy changes.
The driver was Ubuntu 23.10+'s default
`kernel.apparmor_restrict_unprivileged_userns=1` (which makes bwrap unworkable
on stock Ubuntu 24.04 LTS hosts) plus the ecosystem signal (Codex CLI 0.117.0
already shipped Landlock-based sandboxing for the same reason). See
`specs/007-landlock-inference-sandbox/`.

## Cross-cutting integration constraints (for tasks.md ordering)

1. **Migration 25 is shared** by R-2/R-3/R-4/R-5/R-6 — one additive
   transactional migration, not five. `rule_evidence_citations` is shared by
   R-2 and R-6 (define once). `evaluation_results` + reviewer-override record
   (R-4) live in it. **R-2 owns and lands migration 25 first**; R-4/R-5/R-6
   persistence depend on it.
2. **Lifecycle column (R-3) + tombstone (R-2) + reconcile tombstone-awareness
   must ship together** — partial landing re-opens C-5.
3. **R-1 redaction precedes R-2 citation snapshots** (snippets must be captured
   post-redaction) and the legacy-archive content capture.
4. **R-3 removes the auto-write branch; R-6 moves the gate into the same site**
   — coordinate the single `write_rule_files` rework.
5. **R-7 status (`degraded`) is consumed by R-7 UI surfacing** — land the
   storage/status change and the decode together.
6. `min_confidence` → `min_eligibility` rename touches R-3 (inert) and R-6
   (redefines) — one settings change.
7. No NEEDS CLARIFICATION remains; open items are integration sequencing only,
   captured here for `/speckit-tasks`.
