# Internal Contract: `analyze_sessions_stream` (Stream C)

This feature exposes no external/public API. The contract is the internal `learning.rs` stream function and its interaction with the unified inference path. It mirrors the Stream A/B contract so synthesis and downstream consumers are unaffected.

## Function

```rust
async fn analyze_sessions_stream(
    storage: &'static Storage,
    provider: Option<IntegrationProvider>,   // run's provider scope (None = Claude+Codex)
    existing_rules_summary: String,          // for SUPPORT/CONTRADICT/IRRELEVANT verdicts
    app: tauri::AppHandle,                   // for `learning-log` events
    run_id: i64,
) -> (
    Option<StreamFindings>,
    Vec<String>,                              // accumulated stream_log! lines
    Option<crate::cc_client::InferenceCallMetadata>,
)
```

Shape is identical to `analyze_git_stream` (Stream B). Dispatched as the third arm of the existing Phase-1 `tokio::join!`.

## Behavioral contract

1. **Source isolation (FR-001)**: input is derived ONLY from `storage.get_session_breakdown(...)` and the local session index (`sessions_index.get_context` / `search`). MUST NOT spawn `claude /insights` (or any external command) and MUST NOT read `~/.claude/usage-data/**`.
2. **Selection (FR-007, FR-009, FR-013)**: `get_session_breakdown(days = STREAM_C_LOOKBACK_DAYS, hostname = None, provider, limit = STREAM_C_MAX_SESSIONS)`. `provider = None` ⇒ both providers; `Some(p)` ⇒ only `p`. Selection is **cross-project** (no project filter). **Sidechain/sub-agent sessions are filtered out before the `limit` cap is applied** — only top-level user sessions occupy slots. Order is `last_active DESC` (deterministic). Empty result ⇒ return `(None, logs, None)` with a `stream_log!` note (graceful, not an error — FR-008).
3. **Digest + budget (FR-009, FR-012)**: each selected session is reduced to a `SessionDigest.digest` via `prompt_utils::compress_observation`/`safe_truncate`, then a **mandatory secret/credential redaction pass (FR-012)** is applied to that digest before it enters the prompt (in addition to prompt-injection sanitization). The concatenation MUST fit a fixed context budget; sessions with content too thin to digest are skipped (Edge Case), not fabricated.
4. **Extraction**: exactly one `cc_client::invoke_typed::<StreamFindings>(InvokeArgs { phase: Phase::StreamC, prompt, preamble, model: Model::Haiku, max_tokens: 4096 })` call. Output is taken from `structured_output`, or — when the CLI omits it — from JSON extracted out of `result` (the feature-003 defensive parse).
5. **Success**: `(Some(findings), logs, Some(outcome.metadata))`; emit `stream_log!("Stream C: extracted {} patterns, {} verdicts", …)` (FR-010 log-shape parity with A/B).
6. **Failure (FR-005)**: on `InferenceError`, emit `stream_log!("Stream C: …: {e}")` (Display names the specific cause), return `(None, logs, Some(failed_metadata(Phase::StreamC, 4096, &e)))`. Never collapse a specific cause into a generic message at this layer.
7. **No-signal**: model returns `{"patterns": [], "verdicts": []}` ⇒ `Some(StreamFindings{empty})`; treated as "no findings from C" by the decision block (does not, alone, fail the run unless A and B are also empty).
8. **Metadata (FR-006)**: the returned `InferenceCallMetadata` (phase `"stream_c"`, success or failure) is pushed into `inference_metadata_records` at the join site and persisted on `learning_runs.inference_metadata`.

**As-built note (feature 005, 2026-05-18)**: in step 4 (and the synthesis call referenced under "Caller-side contract changes"), the implemented model is `Model::Sonnet46` (pinned `claude-sonnet-4-6`), not `Model::Haiku` — the planned Haiku assignment was superseded for a single-model pipeline with stable cost attribution (feature 005 US5 T060/H-7, L-1). `max_tokens = 4096` and `Phase::StreamC` are unchanged; the original contract text is preserved as the historical record.

## Caller-side contract changes (`learning.rs`)

- **Join site**: replace `gather_insights(...)` with `analyze_sessions_stream(...)`; destructure the 3-tuple; push `insights_metadata` into `inference_metadata_records` (today only obs/git are pushed).
- **Synthesis decision (research R-2)**: generalize to all three `Option<StreamFindings>` —
  - 0 non-empty ⇒ unchanged failure: `"No streams produced findings"`, run `status="failed"`, `Err(msg)`.
  - exactly 1 non-empty ⇒ use it via `to_analysis_output()` (skip Sonnet), `source` label reflects the contributing stream.
  - ≥2 non-empty ⇒ `synthesize_findings(...)` (Sonnet).
- **`synthesize_findings`**: parameter `insights: Option<&InsightsData>` → `insights_findings: Option<&StreamFindings>`; the in-prompt `insights_text` is built by iterating `patterns` (same formatting approach already used for obs/git findings) instead of `InsightsData` fields.
- **Micro mode**: unchanged (Stream A only; Stream C not dispatched).

## Invariants preserved

- Parallel dispatch and `learning-log` line shapes unchanged (FR-010).
- No `cc_client.rs` logic change; `Phase::StreamC` already wired (only its doc comment is refreshed).
- `models.rs`, DB schema, frontend: unchanged.
- Determinism: identical local index state + same `(days, limit, provider)` ⇒ identical session set and digest ordering (supports SC-006 comparison).

## Out of scope (restated)

External `/insights` analytics dimensions (`user_satisfaction_counts`, `claude_helpfulness`, `goal_categories`) and the HTML report are not reproduced. The deterministic structural layer (`session-meta`) is not reproduced — Quill owns it natively. The broader cross-stream silent-failure refactor is deferred (only Stream C's specific-cause reporting is guaranteed here).
