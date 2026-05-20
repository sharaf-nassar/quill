# Phase 1 Data Model: Quill-Native Session Insights Stream

No persisted schema changes. No DB migration. No new public/serialized types. The model is: reuse existing types, add one transient internal struct, delete two obsolete structs.

## Reused (no change)

### `StreamFindings` — Stream C output (`models.rs`)
```
StreamFindings { patterns: Vec<StreamPattern>, verdicts: Vec<RuleVerdict> }
StreamPattern  { name, domain, description, evidence, confidence: f64, is_anti_pattern: bool }
RuleVerdict    { name, verdict, strength: f64 }
```
Derives `Deserialize + Serialize + Clone + Debug + schemars::JsonSchema`. Stream C deserializes into this exactly as Stream A/B do. Validation: `name` is lowercase letters/digits/hyphens (enforced via prompt, same as Stream B); `confidence`/`strength` in [0,1]; `verdict` ∈ {SUPPORT, CONTRADICT, IRRELEVANT}.

### `SessionBreakdown` — selection input (`storage.rs`)
```
SessionBreakdown { provider, session_id, hostname, total_tokens, turn_count,
                   first_seen, last_active, project: Option<String>,
                   has_subagents, subagent_count }
```
Produced by `storage.get_session_breakdown(days, hostname, provider, limit)`, ordered `last_active DESC`. Selection key = `(provider, session_id)`.

### `InferenceCallMetadata` — now emitted for `stream_c` (`cc_client.rs`)
Unchanged shape. After this feature it is populated with `phase = "stream_c"` for every run (success → from envelope; failure → `failed_metadata`), appended to `inference_metadata_records` and persisted on `learning_runs.inference_metadata`. Closes the FR-006 / SC-003 gap (today this entry never exists).

### `LearningRunPayload` / `RunPhase` (`models.rs`/`storage.rs`)
Unchanged. The existing `"streams"` `RunPhase` now counts insights findings alongside obs/git; the synthesis-skipped/failed paths are unchanged in shape.

## Added (internal, transient — not serialized, not persisted)

### `SessionDigest` — per-session compact extraction input
```
SessionDigest {
    provider:   IntegrationProvider,
    session_id: String,
    project:    Option<String>,
    last_active: String,         // RFC3339, for deterministic ordering
    digest:      String,         // bounded, sanitized text: intent + outcome
                                 // + tool/code/command/error signal
}
```
- Built per selected **top-level** `SessionBreakdown` (sidechain/sub-agent sessions excluded, FR-013) from local content (`sessions_index.get_context` / `search`) compressed via `prompt_utils::compress_observation` + `safe_truncate`.
- `digest` is prompt-injection-sanitized (reuses `prompt_utils` sanitation) **and passed through a mandatory secret/credential redaction pass (FR-012)** before use, and bounded so the concatenation of all digests fits the run's context budget (FR-009). Redaction strips literal secrets only and is rule-neutral.
- Lifetime: constructed, concatenated into the Haiku prompt, dropped. Never stored, never returned to the frontend. Exact field set/heuristic finalized at `/speckit-implement` (research R-4).
- **As-built note (feature 005, 2026-05-18)**: the extraction prompt now goes to the pinned `claude-sonnet-4-6` (`Model::Sonnet46`), not Haiku — the planned Haiku assignment was superseded (feature 005 US5 T060/H-7, L-1). Lifetime/budget behavior is unchanged.

## Removed (FR-011)

| Symbol | File | Reason |
|---|---|---|
| `gather_insights` (~150 ln) | `learning.rs` | Replaced by `analyze_sessions_stream` |
| `InsightsData` | `learning.rs` | Adapter for `/insights` facets; Stream C now emits `StreamFindings` |
| `InsightsFacet` | `learning.rs` | Mirrors external `/insights` JSON; no longer read |
| `insight_log!` (local macro) | `learning.rs` | Folded into the Stream B-style `stream_log!` of the new fn |

## Signature changes

| Site | Before | After |
|---|---|---|
| new Stream C fn | `gather_insights(provider, app, run_id) -> (Option<InsightsData>, Vec<String>)` | `analyze_sessions_stream(storage, provider, existing_rules_summary, app, run_id) -> (Option<StreamFindings>, Vec<String>, Option<InferenceCallMetadata>)` (Stream B-shaped) |
| `synthesize_findings` | `insights: Option<&InsightsData>` | `insights_findings: Option<&StreamFindings>` |
| join destructure | `let (insights_result, insights_logs) = …;` (2-tuple) | `let (insights_result, insights_logs, insights_metadata) = …;` (3-tuple) + push metadata |
| synthesis decision | `has_obs`/`has_git` cascade | uniform over `{obs, git, insights}` (research R-2) |

## State / flow

```
get_session_breakdown(days, provider, limit)        ── deterministic, recency-capped, provider-scoped
        │  Vec<SessionBreakdown>
        ▼
per session: local content → compress → SessionDigest ── bounded to context budget (FR-009)
        │  Vec<SessionDigest>
        ▼
invoke_typed::<StreamFindings>(Phase::StreamC, Haiku) ── one call; structured_output → StreamFindings
        │  (Option<StreamFindings>, logs, Option<InferenceCallMetadata>)
        ▼
uniform synthesis decision over {A,B,C}              ── ≥1 non-empty → rules; 0 → fail (unchanged msg)
```

**As-built note (feature 005, 2026-05-18)**: in the flow above, the `invoke_typed::<StreamFindings>(Phase::StreamC, Haiku)` call now uses the pinned `claude-sonnet-4-6` (`Model::Sonnet46`); the synthesis step is also pinned to the same model (single-model pipeline, feature 005 US5 T060/H-7, L-1). The planned Haiku assignment was superseded; the original diagram is preserved as the historical record.
