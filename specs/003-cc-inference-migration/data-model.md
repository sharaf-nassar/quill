# Phase 1 Data Model: Migrate LLM Inference to Claude Code Integration

## Scope

This feature is a transport migration. It touches very little of the existing relational model. The single data-model change is the addition of one column on each run-record table (learning runs, memory-optimization runs) to hold the structured per-call metadata required by FR-016 and SC-009. The model entities used by the inference call sites themselves (`StreamFindings`, `AnalysisOutput`, the memory optimizer's input/output types) are **unchanged in shape**; only their carrier (a `cc_client` invocation rather than a `rig-core` HTTP call) changes.

## New / Modified Entities

### Inference Call Metadata (new entity, persisted as JSON inside `inference_metadata`)

One record per `claude` subprocess invocation made during a run. Multiple records aggregate into a JSON array stored in the parent run's `inference_metadata` column.

| Field | Type | Source | Description |
|---|---|---|---|
| `phase` | string | App-supplied | Synthetic tag identifying which call site fired this invocation: `stream_a`, `stream_b`, `stream_c`, `synthesis`, `memory_optimizer`, `prose_compression`. |
| `model` | string | highest-cost entry of CC envelope `modelUsage` | The primary Claude model id Claude Code used. Agent-mode runs report several models (the requested model plus a cheap tool-loop model); the highest-cost entry is the main generator (e.g. `claude-sonnet-4-6`). Captured because aliases resolve to different concrete models over time. |
| `duration_ms` | u64 | CC envelope `duration_ms` | Total wall-clock the invocation took, as reported by CC. |
| `duration_api_ms` | u64 | CC envelope `duration_api_ms` | Of which time, how much was the API call to Anthropic. |
| `ttft_ms` | u64 | CC envelope `ttft_ms` | Time-to-first-token. |
| `input_tokens` | u64 | CC envelope `usage.input_tokens` | Input tokens billed against the subscription quota. |
| `output_tokens` | u64 | CC envelope `usage.output_tokens` | Output tokens generated. |
| `cache_creation_input_tokens` | u64 | CC envelope `usage.cache_creation_input_tokens` | Tokens charged at the cache-creation rate. |
| `cache_read_input_tokens` | u64 | CC envelope `usage.cache_read_input_tokens` | Tokens served from cache. |
| `total_cost_usd` | f64 | CC envelope `total_cost_usd` | CC's accounting cost in dollars. On OAuth subscriptions this is informational; on BYOK API keys it is the actual bill. |
| `service_tier` | string \| null | CC envelope `usage.service_tier` | E.g. `standard`. Forward-compat field. |
| `stop_reason` | string \| null | CC envelope `stop_reason` | E.g. `end_turn`, `max_tokens`, `tool_use` (latter should not occur given `--tools ""`). |
| `permission_denials` | array | CC envelope `permission_denials` | Should be empty for our invocations; captured so a non-empty array surfaces during debugging. |
| `success` | bool | App-derived | `true` if the invocation produced a usable result; `false` if it failed. Mirrors the variant of `cc_client::InferenceError` (failure variant name stored in `failure_kind` below). |
| `failure_kind` | string \| null | App-derived | One of `claude_code_missing`, `claude_code_too_old`, `not_signed_in`, `rate_limited`, `schema_validation_failed`, `timed_out`, `spawn`, `bad_envelope`, or `null` on success. |

### LearningRunPayload / `learning_runs` row (modified)

Existing fields are preserved verbatim (`trigger_mode`, `observations_analyzed`, `rules_created`, `rules_updated`, `duration_ms`, `status`, `error`, `logs`, `phases`, `provider_scope`). One additive change:

| Field | Type | Description |
|---|---|---|
| `inference_metadata` | TEXT (nullable) | JSON-encoded `Vec<InferenceCallMetadata>` (one element per CC invocation made during this run, ordered chronologically). Null when no invocations occurred (e.g. micro-mode with zero observations). |

### Memory Optimization Run row (modified)

Same additive change: a nullable `inference_metadata` TEXT column.

## Inference Call (clarifying entity from spec)

The spec already defines `Inference Call` and `Inference Failure` at the conceptual level. This data-model document operationalizes them:

- An **Inference Call** is a single invocation of `cc_client::invoke_typed<T>(args)` or `cc_client::invoke_text(args)` where `args` is the existing per-call configuration (model, system preamble, user prompt, max output budget, optional JSON Schema for `T`). The shape of the call site's input does not change across the migration — the new functions take the same logical parameters as `ai_client::analyze_typed` / `ai_client::complete_text`.
- An **Inference Failure** is the categorized failure of one such call, mapped to a `cc_client::InferenceError` variant (R-7 in `research.md`), and recorded as `failure_kind` in the metadata blob above plus a human-readable string in the existing `learning_runs.error` (or memory-optimization equivalent) column.

## Storage migration

A single, lightweight, additive SQLite migration:

```sql
-- Migration N (new):
ALTER TABLE learning_runs ADD COLUMN inference_metadata TEXT;
ALTER TABLE memory_optimization_runs ADD COLUMN inference_metadata TEXT;
-- (The second table name is the actual storage.rs name; verify during task generation.)
```

Both columns are nullable to preserve compatibility with pre-migration rows. No backfill is required — old rows simply have a NULL value, which the UI ignores (no UI surfacing in this feature; SC-009 only requires the data is present going forward).

## Removed entities / persistence

None. The `OAuthHeaderMiddleware`, `AnthropicRateLimitMiddleware`, and rig-core Anthropic client are runtime components, not persisted entities. Removing them does not affect the schema.
