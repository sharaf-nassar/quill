# Phase 0 Research: Quill-Native Session Insights Stream

All NEEDS CLARIFICATION resolved. Decisions are grounded in a read-only audit of `learning.rs`, `cc_client.rs`, `sessions.rs`, `storage.rs`, `prompt_utils.rs`, `models.rs`.

## R-1: Stream C output type — reuse `StreamFindings`

**Decision**: Stream C deserializes into the existing `models::StreamFindings { patterns: Vec<StreamPattern>, verdicts: Vec<RuleVerdict> }` via `invoke_typed::<StreamFindings>` — the exact type Stream A/B use.

**Rationale**: FR-003 mandates "same shape and field-level meaning as the other two extraction streams." `StreamFindings` already derives `Deserialize + Serialize + Clone + Debug + schemars::JsonSchema`, satisfying `invoke_typed`'s bounds. Reuse means synthesis, `to_analysis_output()`, rule persistence, and Wilson scoring consume Stream C with zero special-casing, and no `models.rs` change.

**Alternatives considered**: A bespoke `StreamCInsights` type (the name the 003 contract aspirationally referenced) — rejected: it would force an adapter back into `StreamFindings` for synthesis, contradict FR-003, and add a public type for no behavioral gain.

## R-2: Generalize the synthesis-decision block (fixes FR-004 / US3)

**Decision**: Replace the `has_obs`/`has_git` cascade (`if has_obs && has_git … else if has_obs … else if has_git … else fail`) with a uniform rule over all three `Option<StreamFindings>`: collect the non-empty ones; 0 → fail "No streams produced findings" (unchanged message/behaviour); exactly 1 → use it directly (skip Sonnet, mirroring today's single-stream shortcut); ≥2 → `synthesize_findings` with Sonnet.

**Rationale**: Once Stream C is a `StreamFindings` producer, treating it asymmetrically would re-introduce exactly the defect this feature exists to fix (a run with strong insight signal but empty A/B failing as "no findings"). The uniform rule satisfies FR-004 and US3 directly and *simplifies* the branching.

**Alternatives considered**: Keep insights as synthesis-only context (status quo) — rejected, violates FR-003/FR-004. Always run Sonnet when ≥1 stream — rejected: needlessly costs a Sonnet call when only one stream has signal; today's single-stream shortcut is deliberate and preserved.

## R-3: Deterministic, recency-capped, provider-scoped session selection

**Decision**: Select sessions via `storage.get_session_breakdown(days, hostname=None, provider, limit)` → `Vec<SessionBreakdown>` (ordered `last_active DESC`, filtered by provider, capped by `limit`). Window/cap are named constants (`STREAM_C_LOOKBACK_DAYS`, `STREAM_C_MAX_SESSIONS`); proposed defaults `days = 14`, `limit = 40` (a recent subset, mirroring the prior approach's sampling rather than all history).

Selection is **cross-project** (no project filter — Clarification Q2 = A, FR-007) and **top-level sessions only**: sidechain/sub-agent transcripts are filtered out before the recency cap is applied so they neither occupy slots nor skew the signal (Clarification Q3 = A, FR-013).

**Rationale**: This is already a deterministic, indexed, provider-aware, recency-ordered aggregate (`idx_token_snap_provider_session_ts`). It satisfies FR-007 (provider scope: `provider=None` → both providers; `Some(p)` → that provider; cross-project, matching the prior baseline and Stream A) and the selection half of FR-009 with no new query code. Determinism gives reproducible runs (SC-006 comparability). Sidechain exclusion uses the existing sub-agent attribution (`is_sidechain`/`agent_id`); `SessionBreakdown.has_subagents` only marks parents that delegated and is not itself an exclusion criterion.

**Alternatives considered**: Tantivy `search(query="", sort="recency")` and dedupe by `session_id` — viable but returns per-message hits needing dedupe/aggregation; `get_session_breakdown` is purpose-built and cheaper. Analyze *all* indexed sessions — rejected: unbounded cost/latency (SC-007), and the prior approach itself sampled.

## R-4: Per-session digest assembly + context-budget allocation — **KEY DECISION**

**Decision (direction; exact heuristic finalized at `/speckit-implement`)**: For each selected (top-level) session, build a compact digest from local data — first user prompt (intent/goal), terminal assistant turn (outcome), aggregated tool/code/command/error signal, and error lines prioritized — using `prompt_utils::compress_observation` + `safe_truncate`. **A mandatory secret/credential redaction pass (FR-012, Clarification Q1 = B) runs over each digest before it enters the prompt** — masking API keys, tokens, `.env`-style `KEY=value`, and recognizable credentials while preserving behavioral/semantic text; this is in addition to the existing prompt-injection sanitization. Allocate a fixed total context budget across sessions (per-session cap = `budget / session_count`, floor enforced; drop oldest beyond budget). Concatenate digests into one Haiku prompt. One extraction call per run (no per-session LLM pass). **As-built note (feature 005, 2026-05-18)**: the single extraction prompt now goes to the pinned `claude-sonnet-4-6`, not Haiku (see R-6 as-built; L-1).

**Rationale**: Full transcripts via `get_context()` are richest but blow the context budget at 40 sessions; a bounded per-session digest preserves the FR-002 dimensions (goal/outcome/friction/anti-pattern/summary) while staying within budget and keeping cost/dispatch shape comparable to today (Assumptions). `compress_observation` already prioritizes errors then file paths then outcomes — directly aligned with friction/outcome signal — and is prompt-injection-sanitized.

**This is the main lever on SC-006** and still has open strategy within the fixed constraints above (full-transcript-then-truncate vs. structured-digest vs. search-snippet aggregation; equal vs. recency-weighted budget allocation; redaction implementation — pattern set and masking token). Sidechain handling is no longer open (excluded, FR-013); secret redaction is now mandatory, not optional (FR-012). It remains the implementation contribution point for `/speckit-implement` rather than frozen here.

**Alternatives considered**: (a) Full transcript per session — rejected (budget/latency). (b) Tantivy `SearchHit` snippet aggregation only — viable lower-fidelity fallback, recorded. (c) A separate per-session summarization LLM pass — rejected (extra cost/calls, violates the single-call Assumption).

## R-5: No `cc_client.rs` logic change; refresh the StreamC doc comment

**Decision**: Make no functional change to `cc_client.rs`. `Phase::StreamC` already maps in `as_str()`/`metadata_from_envelope`/`failed_metadata`, so metadata (FR-006) flows automatically once the call exists. Only update the `Phase::StreamC` doc comment (currently "reserved for a future migration … runs `claude /insights --print` directly") to state it is now the active Quill-native path.

**Rationale**: Audit confirmed zero `Phase::StreamC` call sites today and full machinery support. Minimizing blast radius keeps the audited 003 inference surface untouched.

## R-6: Prompt / preamble / model

**Decision**: Mirror Stream B. Preamble: a session-history pattern-analyzer system prompt instructing structured-JSON output matching the schema. Prompt: instruct extraction of 0–N behavioral patterns over the FR-002 dimensions (recurring friction, task outcomes, underlying goals, session types, primary-success / anti-patterns) plus SUPPORT/CONTRADICT/IRRELEVANT verdicts on existing rules, with the empty-output convention `{"patterns": [], "verdicts": []}`. Model `Haiku`, `max_tokens = 4096`, `Phase::StreamC` (identical knobs to Stream A/B).

**Rationale**: FR-002/FR-002a (rule-relevant dimension set, superset of today's effective signal) and consistency with the other extraction streams; Haiku/4096 matches the existing extraction model assignment (FR cost/latency parity, SC-007).

**As-built note (feature 005, 2026-05-18)**: Stream C extraction and synthesis now use the pinned `claude-sonnet-4-6` (`Model::Sonnet46`); the planned `Model::Haiku` assignment was superseded (feature 005 US5 T060/H-7, L-1 — single-model pipeline for stable cost attribution). `max_tokens = 4096` and `Phase::StreamC` are unchanged. The original decision above is preserved as the historical record.

## R-7: Failure surfacing for Stream C (FR-005)

**Decision**: On `invoke_typed` error, follow the Stream A/B pattern exactly: emit a specific `stream_log!("Stream C: …: {e}")` line (the `InferenceError` Display names the specific cause), push `failed_metadata(Phase::StreamC, 4096, &e)`, and return `(None, logs, Some(meta))`. The generalized R-2 decision block still surfaces a specific per-stream cause in the run log/metadata even when the aggregate ends "no findings".

**Rationale**: Satisfies FR-005/SC-005 for Stream C specifically. The broader cross-stream silent-failure refactor remains out of scope (spec Assumptions) — this feature only guarantees specific-cause reporting for Stream C.

## R-8: Deletion set (FR-011)

**Decision**: Delete `gather_insights` (~150 lines), `InsightsData`, `InsightsFacet`, the local `insight_log!` macro. Verify `dirs::home_dir()` / `crate::config::shell_path()` have other callers before assuming removal of imports (audit indicates both are used elsewhere — keep imports). No disabled/unreachable insights path remains.

**Rationale**: FR-001/FR-011 — no learning-pipeline shell-out to the external command and no dead parse path that could mask regressions (the silent-failure class from this session's bug).

## R-9: Testing approach

**Decision**: No automated tests authored proactively (project/user policy: tests only on explicit request). Verification is the `quickstart.md` walkthrough mapped 1:1 to SC-001…SC-007, plus run-record inspection (`learning_runs.inference_metadata` now contains a `stream_c` entry; logs show specific causes). If the user requests tests, target: digest-assembly determinism/bounding (pure fn) and the generalized synthesis-decision matrix.

**Rationale**: Honors the user instruction hierarchy while keeping spec acceptance objectively verifiable.
