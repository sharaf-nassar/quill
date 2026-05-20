# Backend

The Rust backend handles storage, ingestion, search, LLM analysis, plugin management, provider lifecycle management, and the cross-platform status indicator.

It communicates with the frontend through a broad Tauri IPC surface and documented push events.

## Entry Point

[[src-tauri/src/lib.rs]] is the application entry point. It initializes storage, starts the HTTP server, registers all Tauri commands, sets up the tray icon, and launches [[architecture#Background Tasks]].

Tauri plugins configured: `tauri-plugin-log`, `tauri-plugin-updater`, `tauri-plugin-process`, `tauri-plugin-window-state`. Session transcript catch-up is no longer part of app launch; the Sessions window requests an incremental sync when search is opened.

## HTTP API Server

[[src-tauri/src/server.rs]] (995 lines) runs an Axum HTTP server on port 19876 (configurable via `QUILL_PORT` env var) to receive data from external hook scripts.

### Authentication

All endpoints require a Bearer token validated with constant-time comparison (`subtle` crate). The token is generated on first launch by [[src-tauri/src/auth.rs]] and stored at `~/.local/share/com.quilltoolkit.app/auth_secret` with mode 0o600.

### Rate Limiting

Sliding window rate limiter with 60-second buckets. Limits per endpoint type:

| Category | Limit |
|----------|-------|
| General | 100 req/min |
| Observations | 500 req/min |
| Context savings | 500 req/min |
| Session notify | 500 req/min |
| Session messages | 100 req/min |

### Endpoints

The HTTP API exposes 14 endpoints for token ingestion, context savings, learning observations, and session indexing across Claude Code and Codex.

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/api/v1/health` | Health check |
| POST | `/api/v1/tokens` | Record token usage from hook scripts |
| POST | `/api/v1/context-savings/events` | Store context savings events from hooks and MCP tools |
| POST | `/api/v1/learning/observations` | Store tool-use observations |
| GET | `/api/v1/learning/observations` | Retrieve unanalyzed observations |
| GET | `/api/v1/learning/status` | Learning system status |
| POST | `/api/v1/learning/runs` | Record a learning analysis run |
| GET | `/api/v1/learning/runs` | Retrieve learning run history |
| POST | `/api/v1/learning/rules` | Store discovered behavioral rules |
| POST | `/api/v1/sessions/notify` | Notify of new session JSONL file |
| POST | `/api/v1/sessions/messages` | Ingest session messages for indexing |
| GET | `/api/v1/sessions/search` | Full-text search sessions |
| GET | `/api/v1/sessions/context` | Get surrounding messages |
| GET | `/api/v1/sessions/facets` | Get search facets |

Each endpoint validates input (length limits, range checks, type validation) before processing. Provider-aware payloads default legacy callers to `claude`, while new Claude and Codex hooks send explicit provider tags for telemetry and session ingestion. Hook-facing observation and session-ingest POSTs acknowledge after validation and finish SQLite/Tantivy work on background blocking tasks so CLI hooks do not wait on local indexing. Local hook scripts treat receipt of response headers as the success boundary and use a short 1.5-second local timeout, which keeps the CLI path tolerant of slow response teardown without waiting on background indexing.

## Database

[[src-tauri/src/storage.rs]] manages a SQLite database with WAL mode and 5-second busy timeout. The largest backend module.

### Location

The SQLite database file path varies by operating system.

- Linux: `~/.local/share/com.quilltoolkit.app/usage.db`
- macOS: `~/Library/Application Support/com.quilltoolkit.app/usage.db`

### Schema

The database schema is versioned through migration 25 and includes usage, token, context savings, learning, rule governance, session indexing, memory optimizer, code, runtime, and metadata tables.

#### Usage Tracking

Tables for recording and aggregating provider-aware live usage bucket utilization over time.

- **usage_snapshots** — Raw live usage snapshots keyed by provider plus bucket key (timestamp, provider, bucket_key, bucket_label, utilization, resets_at).
- **usage_hourly** — Hourly aggregates keyed by provider plus bucket key (avg/max/min utilization, sample_count). Unique on (hour, provider, bucket_key).

Live usage ingestion stores Claude API buckets and Codex transcript-derived rate-limit buckets in the same tables. Codex `rate_limits.resets_at` values are normalized from transcript epoch timestamps into RFC3339 strings before storage so the live pane can show the same reset countdown semantics as Claude. Migration 14 backfills older Claude-only rows by deriving stable bucket keys from legacy labels, and startup creates provider-only indexes after migrations so older databases can still boot before those columns exist. The generic `settings` table also stores Claude live-usage fetch metadata such as the last attempted poll time, any active 429 cooldown, and the configured indicator primary-provider preference used by the tray and indicator window.

The startup path restores recent live buckets from `usage_snapshots` through [[src-tauri/src/storage.rs#Storage#get_latest_usage_buckets]]. That lookup now uses a grouped latest-timestamp join instead of a correlated subquery because the older form could take tens of seconds once `usage_snapshots` grew large, which left the live pane stuck on `Loading…` during app startup.

#### Token Tracking

Tables for recording per-session token consumption and hourly host-level aggregates with provider provenance.

- **token_snapshots** — Raw token usage per provider/session (provider, session_id, hostname, timestamp, input/output/cache tokens, cwd). Indexed on provider-aware timestamp, session, and cwd paths.
- **token_hourly** — Hourly aggregates per provider/host (total tokens, turn_count). Unique on (hour, hostname, provider).
- Analytics session history, compact token stats, and delete-session cleanup all treat sessions as `(provider, session_id)` pairs so Claude and Codex ids cannot collide.

Migration 20 added `is_sidechain`, `agent_id`, and `parent_uuid` to `token_snapshots` for provider-agnostic sub-agent attribution; the [[backend#Tauri IPC Commands#Usage and Token Commands (13)]] `get_session_breakdown` rollup aggregates across all sidechain rows by `session_id` so a sub-agent's tokens count toward its parent session row. Hook-reported snapshots written before migration 20 stay tagged `is_sidechain=0` (a future CLI repair utility is documented as a TODO in [[src-tauri/src/storage.rs]]).

#### Learning System

Tables for the behavioral learning pipeline: observations, summaries, analysis runs, and discovered rules.

- **observations** — Tool-use observations (provider, session_id, hook_phase, tool_name, tool_input/output, cwd). Indexed on session_id, timestamp, created_at, and provider cleanup paths.
- **observation_summaries** — Per-period/provider/project summaries (tool_counts JSON, error_count, total). Unique on (period, provider, project). Feature 005 (US5 T062, M-1) makes this formerly write-only table readable via `Storage::get_observation_summaries` and folds it into `get_observation_sparkline` as the post-retention historical tail so the trend survives observation pruning; the same change tightens the summary `error_count` from a bare `%error%` substring to a structured-failure-marker predicate (JSON `is_error`/error/status keys, leading `Error:`, runtime panic/traceback banners).
- **learning_runs** — Analysis run records (trigger_mode, observations_analyzed, rules created/updated, duration, status, error, inference_metadata). Feature 005 (US5 T058, H-6) decodes `inference_metadata` tolerantly in `get_learning_runs` into the derived `RunInferenceSummary` rollup on `LearningRun` (no migration — column added by migration 24); NULL/parse-error/empty ⇒ `None`.
- **learned_rules** — Discovered patterns (name unique, domain, confidence, observation_count, file_path, content, state, is_anti_pattern, source). The `content` column (migration 11) stores sanitized rule text for manual promotion. Migration 25 (feature 005) adds governance columns `lifecycle` (persisted lifecycle state, distinct from the read-derived `state` quality label), `origin_run_id`/`origin_model`/`origin_at` (provenance), `current_version`, and `superseded_by`.

Migration 25 also adds six rule-governance tables for the hardened learning loop: **rule_versions** (append-only content history enabling rollback), **rule_evidence_citations** (retention-proof denormalized evidence snapshots grounding a rule), **rule_tombstones** (name-keyed durable suppression that survives re-extraction), **operator_feedback** (per-rule maintainer accept/reject/bad — the primary outcome signal), **evaluation_results** (counterfactual replay verdicts linked to rule + run), and **reviewer_overrides** (audited approval of a regressing rule).

Observation retention (`cleanup_old_observations`) is feature-005-hardened (US5 T061, M-2 / SC-010): the delete cutoff is `MIN(analyzed_watermark, now - 30d)` where `analyzed_watermark = MAX(created_at) FROM learning_runs WHERE status IN ('completed','degraded')`. Observations newer than the watermark have not had an analysis opportunity and are never deleted; with zero completed/degraded runs nothing is deleted at all; the 30-day safety floor only ever adds retention. The summarize-then-delete pair runs in one transaction so a failed summary write rolls back the delete (no more best-effort `.ok()` then unconditional delete).

Startup also creates covering observation indexes for `(created_at, tool_name)` and `(provider, created_at, tool_name)` so learning UI queries such as `get_top_tools` can stay on exact raw-observation windows without paying extra table scans. The same startup pass adds `tool_actions` indexes for `(category, timestamp)` and `(category, provider, session_id)` so ordered code-history lookups and per-session code aggregations avoid broad category scans.

#### Session Indexing

Stores detailed tool invocation and response-time data for MCP-powered session search.

- **tool_actions** — Tool invocation details for MCP (provider, message_id, session_id, tool_name, category, file_path, summary, full_input/output, plus `is_sidechain`, `agent_id`, and `parent_uuid` from migration 20). Indexed on provider/session, message_id, file_path, category, and the new provider+session+sidechain / provider+session+agent pairs. Startup and notify-driven reindexing batch these inserts per extracted message set so analytics queries do not wait behind one transaction per message.
- **response_times** — Assistant response latency per provider/session turn (provider, session_id, timestamp, response_secs, idle_secs, plus the same migration-20 `is_sidechain`/`agent_id`/`parent_uuid` triple). Unique on (provider, session_id, timestamp).

#### Skill Usages

Recognized `SKILL.md` loads derived during the same Session Indexing extraction pass, keyed for analytics drilldowns by skill, provider, project, and host.

- **skill_usages** — One row per recognized skill load (provider, session_id, message_id, skill_name, skill_path, timestamp, tool_name, cwd, hostname). Unique on (provider, session_id, message_id, skill_name, skill_path, timestamp). Indexed on provider+timestamp, provider+session, skill+timestamp, and the migration-22 skill+cwd pair that powers per-project drilldowns. Migration 23 re-arms `skill_usage_reingest_pending` so historical sessions are replayed against the updated extractor without any schema change.

[[src-tauri/src/sessions.rs#extract_skill_accesses_from_tool_action]] recognizes three ingest shapes: Codex `exec_command` calls that read a `SKILL.md` path with `cat`/`head`/`tail`/etc., Claude `Read` calls against a `SKILL.md` path, and Claude `Skill` tool calls. The `Skill` arm normalizes the `skill` input via [[src-tauri/src/sessions.rs#skill_access_from_skill_tool_input]] by stripping any `plugin:` prefix so Claude rows merge with Codex's bare folder names (e.g. Claude `superpowers:using-superpowers` collapses onto Codex `using-superpowers`), and synthesizes a `skill://<raw>` path that preserves the original identifier for forensic drilldowns without colliding with filesystem paths.

`cwd` and `hostname` are populated at ingest from JSONL transcripts: Claude pulls `cwd` from each record's top-level field, Codex threads the session-level `cwd` through every tool message in [[src-tauri/src/sessions.rs#ExtractedMessage]], and the hostname is captured once per batch via [[src-tauri/src/sessions.rs#SessionIndex#local_hostname]] inside [[src-tauri/src/storage.rs#Storage#store_skill_usages_for_messages]]. The HTTP message-ingest path leaves `cwd` null because the payload has no per-message cwd today.

#### Working Context Store

The MCP context store keeps large transient context out of the analytics database.

The Python MCP tools in [[src-tauri/claude-integration/mcp/tools/context.py]] create `~/.config/quill/context/context.db` with `sources`, `chunks`, `executions`, `continuity_events`, `compaction_snapshots`, and `fetch_cache` tables. SQLite FTS5 is used when available, with a LIKE fallback so older SQLite builds still search indexed chunks. Context data stays on the machine running the MCP server.

#### Context Savings Events

The main analytics database stores compact context-savings telemetry from local and remote providers.

- **context_savings_events** — Append-only event records keyed by `event_id`, with provider, session, host, cwd, event type, source, decision, **category**, byte counts, approximate token estimates, refs, and bounded metadata.

Every event carries a `category` from a closed taxonomy: `preservation` (content written to the MCP context store and kept out of the LLM transcript), `retrieval` (LLM pulled preserved content back via `quill_get_context_source` or compaction snapshot read), `routing` (text injected into the transcript by router/capture guidance, search snippets, or bounded `quill_execute` results — these are *transcript cost*, not savings), and `telemetry` (hook observations like `capture.event` and `capture.snapshot` that record session activity but neither leave nor enter the transcript). The canonical mapping lives in [[src-tauri/src/context_category.rs#derive_category]] and is mirrored by `deriveCategory` in `src-tauri/claude-integration/scripts/context-telemetry.cjs` and `_derive_category` in [[src-tauri/claude-integration/mcp/tools/context.py]]; producers set `category` explicitly per call site, the server derives it from `(eventType, decision)` only as a fallback for legacy callers via [[src-tauri/src/context_category.rs#derive_category]], and [[src-tauri/src/storage.rs#backfill_context_event_categories]] applies the same mapping to historical rows during migration 18. Migration 19 re-runs that backfill and zeroes saved/preserved token fields for non-preservation/retrieval rows so stale telemetry producers cannot pollute event-level displays.

The HTTP server accepts batches from context hooks and MCP tools, deduplicates with `INSERT OR IGNORE`, and emits `context-savings-updated`. Analytics queries aggregate by time bucket, provider, category, event type, source, decision, and cwd for the Context tab while leaving large source content in the MCP context store. The shared `CONTEXT_SAVINGS_AGGREGATES_SQL` fragment sums byte and token-indexed/returned columns across every event so breakdown rows still surface router and telemetry traffic, but the saved and preserved token columns inside the same fragment are gated to `category IN ('preservation', 'retrieval')` so capture-hook telemetry contributes zero. The summary path additionally runs `CONTEXT_SAVINGS_CATEGORY_TOTALS_SQL` for the four headline figures (preserved, retrieved, routing, telemetry-event-count) and `CONTEXT_SAVINGS_RETENTION_SQL` to compute `retention_ratio = sources_retrieved / sources_preserved` over distinct `source_ref` values that fall in the active window — both events must be in-window so the ratio stays bounded in `[0, 1]` and reflects engagement rather than pre-window leftovers.

#### Memory Optimizer

Tables for tracking memory files, optimization runs, and actionable suggestions with lifecycle management.

- **memory_files** — Tracked memory files (project_path, file_path, content_hash, last_scanned_at). Unique on (project_path, file_path).
- **optimization_runs** — Optimization run records (project_path, trigger, memories_scanned, suggestions_created, status, timestamps).
- **optimization_suggestions** — Suggestions with lifecycle (run_id FK, action_type, target_file, reasoning, proposed_content, status, backup_data, group_id). Indexed on run_id, project_path+status, group_id.

#### Code and Runtime Metrics

Tables for tracking per-turn LLM response latency and caching git commit history per project.

`get_llm_runtime_stats` groups consecutive rows into logical turns using `idle_secs` to detect tool-execution gaps, then measures each turn's full wall-clock span.

Codex runtime ingestion treats a user prompt as one turn ending at the last assistant or tool-activity timestamp before the next user prompt, because Codex transcripts keep tool calls and outputs on assistant-side records.

Migration 20 also added `is_sidechain`, `agent_id`, and `parent_uuid` to `response_times`. The `idle_secs` turn-grouping logic is unchanged, but each sub-agent forms its own chain of turns scoped by `(provider, session_id, agent_id)` — siblings spawned from the same parent message do not stitch together into a single timeline.

- **git_snapshots** — Cached git history per project (project unique, commit_hash, commit_count, raw_data).

#### Metadata

Key-value configuration and schema migration version tracking.

- **settings** — Key-value config storage.
- **schema_version** — Migration version tracking (currently v25). Migration 20 truncates `response_times` and `tool_actions` (regenerable from transcripts) and sets a `subagent_reingest_pending` flag in `settings`; migration 21 adds `skill_usages` and sets `skill_usage_reingest_pending` so the next [[backend#Session Indexing]] sweep clears `index_state.json` mtimes and re-reads JSONL transcripts to backfill recognized skill-use rows. Migration 22 adds `cwd` and `hostname` columns to `skill_usages` plus the `idx_skill_usages_skill_cwd` index, and re-arms `skill_usage_reingest_pending` so historical rows refill from JSONL transcripts on the next [[backend#Session Indexing]] sweep.

## Tauri IPC Commands

The Tauri commands registered in [[src-tauri/src/lib.rs]] are grouped by feature.

### Usage and Token Commands (13)

Live usage and token analytics commands back provider quota, history, breakdown, and context-savings views.

`fetch_usage_data`, `get_usage_history`, `get_usage_stats`, `get_all_bucket_stats`, `get_snapshot_count`, `get_token_history`, `get_token_stats`, `get_token_hostnames`, `get_host_breakdown`, `get_session_breakdown`, `get_skill_breakdown`, `get_skill_project_breakdown`, `get_context_savings_analytics`.

The live-usage commands now treat utilization history as `(provider, bucket_key)` data instead of assuming a single global Claude bucket label.

Codex live usage now comes from `codex app-server` `account/rateLimits/read` instead of transcript-only scraping. The backend normalizes the returned `rateLimitsByLimitId` map into provider buckets so Quill can store both the base Codex windows and model-specific limits such as Codex Spark in the same usage tables, while preserving the legacy base Codex bucket keys for history continuity. Model-specific `limitName` values are abbreviated for display via [[src-tauri/src/fetcher.rs#abbreviate_codex_model]] (e.g. `GPT-5.3-Codex-Spark` → `5.3-Spark`) by stripping the redundant `GPT-` prefix and `-Codex` infix. The stdio helper resolves the Codex executable path, then augments the user's login-shell `PATH` with the launcher and symlink-target directories so Node-backed npm installs still start from desktop-launched Quill. It ignores unrelated app-server frames such as the `initialize` response, and only deserializes the matching request id for the rate-limit call. If the direct app-server request fails, the fetcher falls back to transcript `token_count` `rate_limits`.

MiniMax live usage comes from the coding plan API at `api.minimax.io` via [[src-tauri/src/fetcher.rs#fetch_minimax_usage]]. It reads the API key from the SQLite settings table and parses the `model_remains` array into 5-hour and weekly `UsageBucket` entries, filtering out models with zero quota.

`get_session_breakdown` now accepts optional provider and limit arguments so Codex live views can request a provider-scoped active set without being crowded out by Claude sessions.

`get_session_breakdown` is provider-agnostic at the row level and rolls up parent + all sub-agent rows for each session: `total_tokens`, `turn_count`, `last_active`, and the input/output/cache columns sum across `is_sidechain ∈ {0, 1}`, and each row carries two new fields — `has_subagents: bool` and `subagent_count: u32` (COUNT DISTINCT `agent_id`) — that gate the [[features#Analytics Dashboard#Now Tab]] expandable tree. The `(provider, session_id, is_sidechain)` index added in migration 20 keeps each `UNION`'d branch on an index scan.

`get_skill_breakdown` returns recognized skill-use counts from the `skill_usages` table for the Analytics Now Skills tab. It accepts the active day range, optional Claude/Codex provider filter, all-time mode, and a capped limit; rows sort by `total_count DESC, skill_name ASC` and include provider sub-counts plus `last_used` and a `project_count` (`COUNT(DISTINCT cwd)`) surfaced in the Skills row metadata in the [[features#Analytics Dashboard#Now Tab]]. The frontend no longer gates expansion on `project_count > 1`, so single-project skills and skills with only null project metadata can open their project drilldown too.

`get_skill_project_breakdown` returns per-(project, hostname) counts for a single skill within the active analytics scope, used by the [[features#Analytics Dashboard#Now Tab]] Skills expand drilldown. It accepts `skill_name`, the active day range, optional Claude/Codex provider filter, all-time mode, and a capped limit; rows sort by `total_count DESC, last_used DESC, project ASC` after applying [[src-tauri/src/storage.rs#compute_subdir_parent_map]] subdir merge so `/a/b/c` folds into `/a/b` exactly like the Projects breakdown. Rows whose transcripts lack `cwd` are preserved as a null-project bucket so expanded counts sum to the parent skill total.

`get_context_savings_analytics` returns range-scoped summary totals, timeseries buckets, grouped breakdowns, and recent append-only events for the Analytics Context tab. Token values are approximate `ceil(bytes / 4)` estimates, while byte counts and event counts are exact where producers can measure them.

### Indicator Commands (3)

`get_indicator_primary_provider`, `set_indicator_primary_provider`, and `get_indicator_state` keep one backend-owned indicator model shared across the tray title, tray summary rows, and the integrations menu.

`set_indicator_primary_provider` persists the configured provider in the settings table, recomputes the resolved indicator state from the shared usage cache or fallback rows, and emits `indicator-updated` so the tray summary and integrations menu stay synchronized without a second polling path.

### Project and Session Management (7)

`get_project_tokens`, `get_session_stats`, `get_project_breakdown`, `delete_project_data`, `rename_project`, `delete_host_data`, `delete_session_data`.

`delete_session_data` deletes both token snapshots and skill-use rows for the selected `(provider, session_id)` pair so the Sessions and Skills breakdowns cannot retain different views of a deleted session.

### Integration Commands (12)

Commands for detecting providers and running install/uninstall flows, plus per-provider and global feature toggles.

Provider setup state is persisted through the settings table using key `integration.providers.v1` to survive app restarts. Three global feature flags — `context_preservation.enabled` (default false), `feature.activity_tracking.enabled` (default true), and `feature.context_telemetry.enabled` (default true, gated on context preservation) — drive which optional Quill assets get deployed into Claude Code and Codex.

The `confirm_enable_provider` command accepts an optional `api_key` parameter used by service-only providers like MiniMax and reads the global `IntegrationFeatures` from storage so newly-enabled providers inherit the current feature set automatically. `get_context_preservation_status` also reports whether historical context-savings events exist so the Analytics Context tab can remain visible after the feature is disabled.

`rescan_integrations` drops the cached login-shell PATH (see [[src-tauri/src/config.rs#refresh_shell_path]]) and re-runs detection so users who edit their shell config or install a CLI mid-session can pick it up without restarting Quill. Failed CLI detections persist the candidate paths inspected on `ProviderStatus.lastDetectionAttempts` so the integrations menu can show why a provider is "N/A" despite being installed.

`set_minimax_api_key` updates a stored MiniMax API key in place (no disable/re-enable round-trip) and emits `integrations-updated`.

`get_integration_features` returns the resolved `IntegrationFeatures` struct. `set_activity_tracking_enabled`, `set_context_telemetry_enabled`, and `set_brevity_enabled` each save their flag, reinstall every currently-enabled provider via [[src-tauri/src/integrations/manager.rs#apply_features_to_enabled_providers]] (which also re-syncs brevity blocks via `sync_brevity_blocks`), and emit `integration-features-updated`. The existing `set_context_preservation_enabled` follows the same path so all four feature toggles share one sync function.

`get_provider_statuses`, `rescan_integrations`, `confirm_enable_provider`, `confirm_disable_provider`, `get_context_preservation_status`, `set_context_preservation_enabled`, `set_minimax_api_key`, `get_runtime_settings`, `set_runtime_settings`, `get_integration_features`, `set_activity_tracking_enabled`, `set_context_telemetry_enabled`, and `set_brevity_enabled`.

At startup, [[src-tauri/src/integrations/manager.rs]] verifies enabled, detected Claude and Codex providers against the stored context-preservation setting. Missing or stale Quill-managed hooks, MCP assets, templates, or unexpectedly present context assets trigger an idempotent reinstall of either the base-only or context-enabled asset set; repair failures leave the provider enabled but persist `last_error` and an error setup state.

### Runtime Settings Commands (2)

Single IPC pair backing the [[features#Settings Window]]'s Performance, General (always-on-top), and Learning (rule watcher) tabs.

`get_runtime_settings` returns the resolved `RuntimeSettings` struct with `live_usage.enabled`, `live_usage.interval_seconds`, `plugin_updates.enabled`, `plugin_updates.interval_hours`, `rule_watcher.enabled`, and `always_on_top` clamped to safe ranges (live: 60–600s, plugin updates: 1–24h). `set_runtime_settings` persists each key, calls `WebviewWindow::set_always_on_top` on the main window when that flag changes, and emits `runtime-settings-updated` so any open Settings window observes the resolved values without a re-fetch.

### Learning Commands (18)

Commands for managing the behavioral learning pipeline settings, rules, and observations.
Read and trigger commands accept an optional provider filter so the UI can request Claude-only, Codex-only, or combined learning views.

`get_learning_settings`, `set_learning_settings`, `get_learning_capability`, `get_learned_rules`, `delete_learned_rule`, `promote_learned_rule`, `rollback_rule`, `reactivate_rule`, `submit_rule_feedback`, `record_reviewer_override`, `run_rule_evaluation`, `get_learning_runs`, `trigger_analysis`, `get_observation_count`, `get_unanalyzed_observation_count`, `get_top_tools`, `get_observation_sparkline`, `read_rule_content`.

State-changing learning commands are authorized (feature 005 US2 — H-4 / FR-011, see `specs/005-learning-system-hardening/contracts/ipc-and-feedback.md`). At startup the backend mints an ephemeral per-process capability token (`OsRng`, held only in Tauri managed state, never persisted). `get_learning_capability` returns it ONLY to the window whose label is `learning`. A single reusable guard runs first on every mutating command — constant-time token compare via the `subtle` crate plus a `learning`-window-label assertion — and is applied to `delete_learned_rule`, `promote_learned_rule`, `rollback_rule`, `reactivate_rule`, `submit_rule_feedback`, `record_reviewer_override`, and `run_rule_evaluation` (each gains a `token` arg). All three `submit_rule_feedback` values (`accept`/`reject`/`bad`) are guarded — `bad` writes a durable tombstone and changes active state, while `accept`/`reject` carry the same trust as promote/delete per the contract (feature 005 US3 — FR-029). `record_reviewer_override` (feature 005 US4 — FR-020) writes one audited `reviewer_overrides` row via [[src-tauri/src/storage.rs#Storage#record_reviewer_override]] (reason required, non-empty; `overridden_by` = the authorized window label) — the only way to approve a rule whose latest counterfactual verdict regresses the replay set. `run_rule_evaluation` (feature 005 US4 — V5/FR-019) is the in-app trigger for the otherwise-unreachable [[src-tauri/src/eval_harness.rs]]: it builds the [[src-tauri/src/eval_harness.rs#RuleUnderTest]] via [[src-tauri/src/storage.rs#Storage#eval_inputs_for_rule]], `await`s `eval_harness::run_evaluation` (NOT wrapped in `run_blocking`), attributes the verdict to the latest `completed|degraded` run, persists one `evaluation_results` row via [[src-tauri/src/storage.rs#Storage#persist_evaluation_result]], and returns a compact summary (verdict + warn-not-block cautions). Read commands (`get_learned_rules`, `read_rule_content`, `get_learning_runs`, …) stay unauthenticated. The HTTP `POST /api/v1/learning/rules` ingest keeps its bearer auth and is clamped to `lifecycle='candidate'` — its payload carries no lifecycle field and `store_learned_rule` is structurally incapable of producing `awaiting_review`/`active`.

`get_top_tools` intentionally reads exact raw-observation windows instead of reusing `observation_summaries`, because summary rows are keyed by cleanup period rather than original event timestamps. The backend relies on the covering observation indexes above to keep that exact-window query responsive.

### Code and Response Stats (5)

`get_code_stats`, `get_code_stats_history`, `get_batch_session_code_stats`, `get_llm_runtime_stats`, `get_session_subagent_tree`.

`get_batch_session_code_stats` fans out one SQL branch per `(provider, session_id)` pair with `UNION ALL` so SQLite can use the `tool_actions` provider/session index instead of falling back to a broad category scan across the entire code-change corpus.

`get_llm_runtime_stats(range, scope)` accepts an optional `scope: "all" | "parent_only"` argument. `None` or `"all"` preserves the existing behavior across every row; `"parent_only"` adds `WHERE is_sidechain = 0` so the headline runtime card on the [[features#Analytics Dashboard#Now Tab]] can report parent-thread cost without sub-agent traffic inflating it. The `(provider, session_id, is_sidechain)` index covers the filter.

`get_session_subagent_tree(provider, session_id) -> Vec<SubagentNode>` is lazy-fetched by the [[features#Analytics Dashboard#Now Tab]] only after a sub-agent-bearing session row is expanded. Implementation in [[src-tauri/src/storage.rs#Storage#get_session_subagent_tree]] returns one node per `agent_id` for the requested `(provider, session_id)`, carrying `parent_agent_id` (null at depth 1; populated when Claude later spawns depth-2+ sub-agents and rebuilt at query time from `parent_uuid` chains), `first_seen`/`last_active`, `turn_count`, the input/output/cache/total token breakdown, `tool_call_count`, and a reserved `label: Option<String>` (always `None` today).

### Memory Optimizer Commands (16)

Commands for managing memory files, optimization runs, and suggestion approval workflows.
Most read and trigger commands accept an optional provider filter for Claude, Codex, or combined views.

`get_memory_files`, `trigger_memory_optimization`, `get_optimization_suggestions`, `approve_suggestion`, `deny_suggestion`, `undeny_suggestion`, `undo_suggestion`, `approve_suggestion_group`, `deny_suggestion_group`, `get_suggestions_for_run`, `get_optimization_runs`, `get_known_projects`, `add_custom_project`, `remove_custom_project`, `delete_memory_file`, `delete_project_memories`.

### Plugin Commands (14)

Commands for installing, updating, enabling, and managing plugins and marketplaces.
All plugin commands take a provider argument so the frontend can target Claude or Codex explicitly while keeping one shared window.

`get_installed_plugins`, `get_marketplaces`, `get_available_updates`, `check_updates_now`, `install_plugin`, `remove_plugin`, `enable_plugin`, `disable_plugin`, `update_plugin`, `update_all_plugins`, `add_marketplace`, `remove_marketplace`, `refresh_marketplace`, `refresh_all_marketplaces`.

Claude plugin mutations delegate to the `claude plugin` CLI and marketplace git repos. Codex plugin reads and install/remove operations use resolved `codex app-server` JSON-RPC over stdio with the launcher-aware execution `PATH`, while unsupported Codex mutations return provider-specific errors instead of guessing behavior.

### Session Indexing Commands (4)

`search_sessions`, `get_session_context`, `get_search_facets`, and `sync_search_index` all operate on a unified Claude-plus-Codex index. Search and context requests include provider identity so session collisions do not bleed across providers.

`sync_search_index` runs an mtime-based incremental sweep — not a wipe-and-rebuild — so a true rebuild requires deleting the on-disk index dir while the app is closed (or bumping `SCHEMA_VERSION` in [[src-tauri/src/sessions.rs]]).

### Restart Commands (7)

`discover_restart_instances`, `discover_claude_instances` (compat alias), `request_restart`, `cancel_restart`, `get_restart_status`, `install_restart_hooks`, `check_restart_hooks_installed`.

Restart commands expose a shared provider-aware row model across Claude and Codex. Hook install/check commands accept an optional provider parameter so restart setup can be applied per provider.

### UI Commands (4)

`hide_window`, `quit_app`, `install_app_update`, `get_release_notes`.

[[src-tauri/src/lib.rs#install_app_update]] re-checks the configured updater from Rust, downloads and installs the release, logs the resolved relaunch binary, then schedules a detached relaunch via [[src-tauri/src/lib.rs#spawn_delayed_relaunch]] and exits the primary so the titlebar update button shares the backend-owned install-and-relaunch boundary with the tray updater. The detached relaunch is required because `tauri-plugin-single-instance` would otherwise treat the new process as a duplicate launch (see [[architecture#Architecture#Single Instance]]).

[[src-tauri/src/lib.rs#get_release_notes]] proxies the public GitHub releases API for `sharaf-nassar/quill` via [[src-tauri/src/releases.rs#fetch_release_notes]], drops drafts and prereleases, and returns a normalized `ReleaseNote` list (tag, name, body, html url, published_at) that the [[frontend#Frontend#Components]] release-notes window paginates with Previous/Next. The command takes an optional `limit` (clamped to 1-100, default 30) so the frontend can request a small newest-first window without exposing GitHub pagination details. Unauthenticated requests are used because the repository is public; rate-limit and HTTP errors are surfaced as `Result::Err` strings rather than swallowed.

### Integration Commands (9)

Integration IPC exposes provider detection, manual rescan, provider enablement, the global context-preservation toggle, the global brevity toggle, and the in-place MiniMax API-key update.

`get_provider_statuses`, `confirm_enable_provider`, `confirm_disable_provider`, and `get_context_preservation_status` expose provider state and the context-preservation setting. `set_context_preservation_enabled` installs or removes local context-preservation assets for currently enabled Claude and Codex providers without deleting historical context data.

`get_provider_statuses` returns the last saved provider statuses from storage rather than re-running detection. Fresh detection happens once at startup via the background `startup_refresh` task, which saves results and emits `integrations-updated`. This avoids redundant subprocess calls and eliminates the visible "Checking integrations..." loading state on the main window.

`rescan_integrations` is the explicit retry path: it calls [[src-tauri/src/integrations/manager.rs#force_rescan]] (which clears the cached login-shell PATH and dynamic-prefix cache via [[src-tauri/src/config.rs#refresh_shell_path]] and reruns `startup_refresh`), then invalidates and re-warms the usage cache so a previously-N/A provider that just flipped to detected is reflected in the tray indicator without waiting for the next polling cycle. Used by the integrations menu's "Rescan PATH" button when the user has just installed a CLI or edited shell config.

Detection runs via `--version` checks for CLI providers through the shared [[src-tauri/src/config.rs#detect_provider_cli]] helper, which both `claude::detect` and `codex::detect` delegate to so a single fix to PATH augmentation, error handling, or timeouts covers both providers. The shared resolver in [[src-tauri/src/config.rs#resolve_command_path]] layers a login-shell `command -v` lookup with a static fallback list (bun, cargo, deno, volta, npm-global, n, asdf, mise, nodenv, Nix profile, yarn classic, `~/.claude/local/`, Linuxbrew, Homebrew, MacPorts, snap) and dynamic `npm config get prefix` / `bun pm bin -g` / `yarn global bin` queries — covering installs whose dirs only appear in interactive shell config (`~/.zshrc`) which `zsh -lc` does not source. Dynamic-prefix outputs are validated against a trusted-roots allow-list before being added to the candidate list so a malicious `npm config set prefix /tmp/evil` cannot trick Quill into executing an attacker-controlled binary. Failed detections record every path inspected on `ProviderStatus.lastDetectionAttempts` with `$HOME` redacted to `~/...` (and the field skipped from JSON when empty) so the integrations menu can show a per-row diagnostic tooltip without leaking the local username. Service-only providers like MiniMax skip CLI detection and use API key presence instead. Implementation lives in [[src-tauri/src/integrations/mod.rs]], [[src-tauri/src/integrations/claude.rs]], [[src-tauri/src/integrations/codex.rs]], and [[src-tauri/src/config.rs]].

## Event System

The backend pushes real-time updates to the frontend via Tauri's emit system.

| Event | Source | Payload | Trigger |
|-------|--------|---------|---------|
| `tokens-updated` | server.rs | `()` | Token snapshot stored |
| `context-savings-updated` | server.rs | `()` | Context savings events stored |
| `learning-log` | learning.rs | `{run_id, message}` | Real-time analysis progress |
| `learning-updated` | lib.rs | `()` | Rules changed |
| `plugin-changed` | lib.rs | `()` | Plugin or marketplace mutation completed |
| `plugin-bulk-progress` | plugins.rs | `BulkUpdateProgress` | Per-plugin update progress |
| `plugin-updates-available` | plugins.rs | `count` | New updates found |
| `provider-status-updated` | integrations | `Vec<ProviderStatus>` | Startup provider detection refresh |
| `restart-status-changed` | restart.rs | `RestartStatus` | Restart phase change |
| `integrations-updated` | integrations/manager.rs | `ProviderStatus[]` | Startup refresh or provider enable/disable completed |
| `context-preservation-updated` | integrations/manager.rs | `ContextPreservationStatus` | Global context-preservation toggle changed |
| `integration-features-updated` | integrations/manager.rs | `IntegrationFeatures` | Activity tracking or context telemetry toggle changed |
| `runtime-settings-updated` | lib.rs | `RuntimeSettings` | Live-usage / plugin-update / rule-watcher / always-on-top toggle changed |
| `ui-prefs-updated` | useUiPrefs (frontend) | `UiPrefs` | Layout, time mode, or panel-visibility preference changed in the Settings window |
| `indicator-updated` | lib.rs | `StatusIndicatorState` | Shared usage refresh or primary-provider change recomputed indicator state |
| `memory-optimizer-log` | memory_optimizer.rs | `{message}` | Optimization run progress |
| `memory-optimizer-updated` | memory_optimizer.rs | `{run_id, status}` | Run completed |
| `memory-files-updated` | memory_optimizer.rs | `{project_path}` | Memory files changed |

Indicator state payloads use the explicit status vocabulary `ready`, `degraded`, or `unavailable` so the frontend can treat healthy state distinctly from warning and empty-state cases without legacy `ok` handling.

## Session Indexing

[[src-tauri/src/sessions.rs]] provides full-text search over Claude Code and Codex transcripts using Tantivy, with provider-safe identity for indexing, search hits, context lookup, and reindex cleanup.

### Index Schema

The Tantivy index stores provider, identity, content, and enrichment fields for shared session search.

Fields include provider, message_id, session_id, content, role, project, host, timestamp, git_branch, tools_used, files_modified, code_changes, commands_run, tool_details, and a stored display text field. Provider/project/host are faceted for filters. Stored at `~/.local/share/com.quilltoolkit.app/session-index/`.

### Indexing Strategy

Session Search triggers an incremental mtime scan of `~/.claude/projects/` and `~/.codex/sessions/**` before loading facets, while hook-driven notify/message ingestion keeps the index fresh during app runtime.

When a transcript is reprocessed, Quill now coalesces repeated `notify` requests per session and applies each rewrite under one Tantivy writer lock with a single commit. This avoids overlapping delete-and-reindex batches while keeping SQLite `tool_actions` and `skill_usages` writes batched per extracted file or payload. The mtime sweep deletes existing session docs unconditionally before reinserting, even on first sight of a file, so hook-driven `notify` ingestion that ran before the file was tracked in `index_state.json` cannot stack duplicate copies on top.

Skill usage is derived during the same extraction pass by [[src-tauri/src/sessions.rs#extract_skill_accesses_from_tool_action]], which recognizes read-like loads of a `SKILL.md` file and derives the skill name from that file's parent directory. The indexer deletes and replaces `skill_usages` per `(provider, session_id)` so repeated transcript walks do not duplicate counts, and it does not infer skills from assistant prose, available-skill lists, or skill-file maintenance edits.

The Claude walker descends into `<projectSlug>/<session-uuid>/subagents/agent-*.jsonl` in addition to the flat parent transcript at `<projectSlug>/*.jsonl`, and each `DiscoveredSessionFile` carries an `is_subagent` flag so downstream extraction can tell the two apart. Claude extraction reads `isSidechain`, `agentId`, and `parentUuid` from each JSON record; Codex extraction writes the provider-agnostic defaults (`is_sidechain=0`, `agent_id=NULL`, `parent_uuid=NULL`) so today's Codex CLI inherits the same code path the day OpenAI ships a sub-agent feature. Per-session sub-agent files live in one flat directory — multi-level hierarchy is reconstructed at query time via `parent_uuid` chains rather than nested filesystem layout.

The HTTP API also accepts provider-tagged notify and direct message ingestion. Local Claude full-transcript sync is Stop-scoped, while direct message ingestion still appends atomically for incremental remote updates. BM25 scoring plus snippet generation power the shared search UI with provider filters and badges.

### Search Scoring

Query parsing applies per-field BM25 boosts so concrete artifacts outrank noisy metadata.

The default-search field weights are: `files_modified` (4.0), `code_changes` (2.5), `commands_run` (2.5), `tool_details` (1.5), `content` (1.0), and `tools_used` (0.5). Without these boosts, equal weighting plus BM25 length-normalization let short fields like `tools_used` (where every session contains tokens like `Edit` and `Bash`) dominate ranking. The derived `display_text` field is kept in the parser at boost 0.1 only so Tantivy's `SnippetGenerator` — which filters terms by field — can highlight matches against it; it is a superset of `content + code_changes + commands_run + tool_details` and would otherwise double-count every term. Query-parser errors from `parse_query_lenient` are logged at debug level instead of being silently discarded.

## Claude Code Inference Client

[[src-tauri/src/cc_client.rs]] is the single inference surface for the app. Every LLM call (learning streams + synthesis, memory optimizer, prose compression) spawns the `claude` CLI in headless one-shot mode rather than making a direct HTTP request to Anthropic.

Public surface: [[src-tauri/src/cc_client.rs#invoke_typed]] for schema-validated structured output and [[src-tauri/src/cc_client.rs#invoke_text]] for free-form prose. Model routing: Sonnet 4.6 (pinned by full model name `claude-sonnet-4-6`) for all work — pattern extraction, learning synthesis (single-model since feature 005 US5 T060/H-7; no rolling `sonnet` alias, stable cost attribution), and prose work. The `Haiku`/`Sonnet` enum variants are retained dead-code-only for easy revert. `--json-schema` is unreliable (the CLI does not enforce it), so typed calls do not use it. `invoke_typed` instead embeds the JSON Schema in the prompt, grants the headless agent a `Write`-only tool sandboxed to a per-call temp dir, and has it write the result to `out.json`; Quill reads that file and `serde_json::from_str::<T>` is the sole validation (missing/invalid → `SchemaValidationFailed`, no app-side retry). `invoke_text` is unchanged (free-form, total tool isolation).

The `claude` binary is located via [[src-tauri/src/config.rs#resolve_command_path]] — the same cached, login-shell-aware resolver used for provider CLI detection — so it picks up Anthropic's `claude migrate-installer` target and auto-refreshes when the user triggers a PATH rescan. Each invocation runs `claude -p --output-format json --model <alias> --append-system-prompt <preamble> --tools "" --disable-slash-commands --no-session-persistence --setting-sources "" --exclude-dynamic-system-prompt-sections` and pipes the prompt body on stdin, joined with `wait_with_output` in a single future so a large prompt cannot deadlock against the child's stdout. The subprocess is isolated from the user's interactive Claude Code configuration (their hooks, slash commands, plugins, CLAUDE.md auto-discovery, and session history are all suppressed) and runs with `CLAUDE_CODE_*`, `ANTHROPIC_*`, and `NODE_OPTIONS` scrubbed from the inherited environment.

No app-side retry, no `Retry-After` interpretation, no rate-limit backoff. Each invocation has a 300-second hang-detector timeout (via `tokio::time::timeout` + `kill_on_drop`). Errors are categorized into eight stable variants — `ClaudeCodeMissing`, `ClaudeCodeTooOld`, `NotSignedIn`, `RateLimited`, `SchemaValidationFailed`, `TimedOut`, `Spawn`, `BadEnvelope` — each producing a user-facing message that names the cause and the actionable remediation. See `specs/003-cc-inference-migration/contracts/cc-client.md` for the full contract.

On top of the in-process flag isolation (defense in depth, kept verbatim), the spawned `claude` is wrapped with the best-available OS-level confinement because it processes untrusted captured content. [[src-tauri/src/cc_client.rs#apply_sandbox]] runs as the last step of `build_command`, rewrapping the fully-formed command. Linux is a three-tier chain — **Landlock** (primary; in-process kernel LSM, no user namespaces, no AppArmor entanglement) → **Bwrap** (subprocess fallback for kernels without Landlock or hosts where bwrap is still permitted) → **None** (unconfined, honestly recorded, actionable diagnostic emitted). macOS uses `sandbox-exec` with a deny-by-default SBPL profile (reads scoped to system/runtime prefixes + the resolved claude/node tree, **no** `$HOME`/`~/.claude`/`~/.config`/project access; writes confined to the per-call temp dir); Windows relies on the existing `kill_on_drop` Job Object association (documented best-effort). The Linux primary tier applies a Landlock ruleset built by [[src-tauri/src/cc_client.rs#build_ruleset]] from a [[src-tauri/src/cc_client.rs#LandlockPolicy]] (ABI v3 declared with `CompatLevel::BestEffort` so older Landlock-capable kernels degrade access rights cleanly) via a forked-child pre-spawn hook on the `tokio::process::Command`'s underlying `std::process::Command::pre_exec` — the hook runs `prctl(PR_SET_NO_NEW_PRIVS, 1, …)` then `RulesetCreated::restrict_self()` in the child between `fork` and `execve` so Quill itself stays unrestricted; the ruleset grants RO `path_beneath` rights to `{/usr, /bin, /sbin, /lib, /lib32, /lib64, /etc, /opt, /nix-if-present, /proc, /sys, /dev, claude_install_root}` and RW rights to `{per-call TempDir, /dev/null}`, with absent optional paths silently skipped (mirrors bwrap's `--ro-bind-try`). The host pseudo-filesystems `/proc`, `/sys`, `/dev` are in the RO set because Landlock has no mount namespace (unlike bwrap's `--proc`/`--dev`/`--tmpfs` which inject fresh ones) — denying them makes the launcher's Bun runtime SIGILL at startup on `readlink(/proc/self/exe)` / `open(/dev/urandom)` / `open(/proc/cpuinfo)`; the trade-off vs bwrap is that `/proc/N/*` exposes other PIDs' cmdline/environ to the subprocess (the spec's `$HOME`/`~/.claude`/`~/.config`/project-path denial is preserved). [[src-tauri/src/cc_client.rs#build_command]] also exports `TMPDIR=<per-call dir>` and `NODE_COMPILE_CACHE=<per-call dir>` on the typed-call path so the launcher's transient writes route into the already-allowed RW dir instead of `/tmp` (no-op under bwrap, which gives a private tmpfs `/tmp`, and under `None`). The Bwrap fallback's argument construction is byte-for-byte the same as before feature 007 (deny-by-default filesystem, no `$HOME`/`~/.claude`/`~/.config`/project access, a single RW bind of the per-call temp dir); only its *position* in the chain moved from primary to first fallback. The previous `unshare`-based `ProcessOnly` tier introduced by feature 006-A is **retired** — it required the same `CLONE_NEWUSER` capability AppArmor blocks on Ubuntu 24.04+, so it was theatrical on exactly the hosts that broke bwrap, with no FS-confinement value either way. When the chain falls all the way through to `None`, a process-wide one-shot diagnostic ([[src-tauri/src/cc_client.rs#emit_no_confinement_diagnostic]], guarded by `OnceLock<()>`) is emitted to both `log::error!` (visible in the `tauri dev` terminal) and the per-call log channel that lands in `learning_runs.logs` (visible in run-history detail) — two templates: a **generic FR-014** message when neither mechanism is available at detection, and an **AppArmor-specific FR-015** message when bwrap was attempted and failed because of Ubuntu 23.10+'s default `kernel.apparmor_restrict_unprivileged_userns=1` policy (detected by [[src-tauri/src/cc_client.rs#classify_bwrap_failure]] returning [[src-tauri/src/cc_client.rs#BwrapBrokenCause]]`::AppArmorRestrictUserns` after substring-matching bwrap stderr against `"setting up uid map: Permission denied"` or `"loopback: Failed RTM_NEWADDR: Operation not permitted"`); a process-wide `OnceLock<BwrapBrokenCause>` latch prevents re-spawning the same known-broken bwrap on subsequent calls in the same Quill process. Network is deliberately preserved on every branch (no net namespace / `network-outbound` allowed, no Landlock network rules) — the CLI makes the model API call itself. Helper binaries are still resolved via a `std::env::split_paths` PATH scan plus absolute fallbacks; the one new approved dependency is `landlock` 0.4.4 (Apache-2.0/MIT, kernel-feature author's crate, Linux-only target-cfg). Confinement **never fails closed**: if Landlock build/probe errors, the chain falls through to bwrap; if bwrap is absent or latched-broken, the flag-isolated command runs unchanged and inference continues; the reduced state is recorded. See `specs/005-learning-system-hardening/research.md` R-7.6, `specs/006-learning-hardening-followups/research.md` R-A, `specs/007-landlock-inference-sandbox/research.md` R-A..R-F, `specs/007-landlock-inference-sandbox/contracts/landlock-sandbox.md` C-A..C-E, and FR-005/SC-013.

The structured `--output-format json` envelope returned by every call carries per-call metadata (input/output tokens, cache stats, model id, durations, cost, stop reason, permission denials) that is captured into [[src-tauri/src/cc_client.rs#InferenceCallMetadata]] and persisted on the parent run record's `inference_metadata` JSON column for both `learning_runs` and `optimization_runs`. The record also carries a `sandbox` field — one of the closed write vocabulary `{"landlock", "bwrap", "sandbox-exec", "job-object", "none"}` ([[src-tauri/src/cc_client.rs#SandboxKind]]) — recording the applied OS confinement for every call on both the success and `failed_metadata` paths so SC-013 (confinement state recorded for 100% of analysis runs on every platform) is verifiable. The tag is honest about the boundary: [[src-tauri/src/cc_client.rs#sandbox_tag_is_fs_confined]] (single source of truth, keyed on the stable tag) is `true` for `landlock`/`bwrap`/`sandbox-exec` (real deny-by-default filesystem confinement) and `false` for `job-object`/`none`; the classifier stays **tolerant of any legacy tag** including the retired `"process-only"` and pre-feature-006 `"unshare"` (both → `false`) and any unknown future tag (→ `false`), so historical rows decode forever without migration (feature 007 contract C-D). Feature 006 Follow-up A's operator-disclosure plumbing is preserved unchanged: [[src-tauri/src/storage.rs#decode_inference_metadata]] projects a derived `confinement` (`{ sandbox, fs_confined }`) onto each `RunInferenceCall` and an `all_fs_confined` rollup onto `RunInferenceSummary`, and [[src/components/learning/RunHistory.tsx]] renders a distinct amber marker plus the remediation hint for any run that recorded a not-FS-confined call (FS-confined and legacy/no-inference runs render unchanged).

[[src-tauri/src/fetcher.rs]] is the only remaining consumer of the Claude Code OAuth credential in the codebase. It powers the [[features#Live Usage View]] by polling `api.anthropic.com/api/oauth/usage` and was intentionally not migrated as part of feature 003 (see `specs/003-cc-inference-migration/spec.md` FR-015).

## Git Analysis

[[src-tauri/src/git_analysis.rs]] (343 lines) extracts commit patterns for the [[features#Learning System]].

Collects commit messages, file hotspots (change frequency), co-change patterns (files changed together), and directory structure. Excludes merge commits (>20 files) and minified code. Results cached by project + HEAD commit hash, invalidated on HEAD change. Compressed to 4,500 bytes for LLM context. Commit lines are prefixed with the git `%h` short-hash and the compressed block leads with a `[SNAPSHOT HEAD <hash>]` key (feature 005 US3 T040, H-1) so Stream B can emit resolvable `kind="commit"` evidence refs that [[src-tauri/src/storage.rs#Storage#resolve_evidence_refs]] verifies via `git cat-file` or the `git_snapshots` cache; redaction still runs before compression so the cache stays secret-free.

Every git-derived text field (commit subjects, hotspots, diff stats, folder structure, and per-commit co-change file lists) is passed through [[src-tauri/src/redaction.rs#redact]] before `compress_git_data` truncates and before the result is written to the `git_snapshots.raw_data` cache. The cached value and the prompt value are therefore both redacted, so a cache hit cannot re-leak a secret.

## Concurrency

The backend uses Tokio for async operations with specific patterns:

- `tokio::task::block_in_place()` for sync DB/file operations within async context
- `tokio::spawn()` and `tauri::async_runtime::spawn()` for background tasks
- `parking_lot::Mutex<T>` for fast synchronization (preferred over `std::sync::Mutex`)
- `parking_lot::RwLock<T>` for invalidatable single-writer caches (e.g. the login-shell PATH cache in [[src-tauri/src/config.rs]]) where `std::sync::RwLock` would risk poisoning across the process on writer panic
- `Arc<T>` for shared ownership across task boundaries
- `OnceLock<T>` for one-time initialization of globals (STORAGE, HTTP_CLIENT) — used only for caches that never need to be invalidated; invalidatable caches use `parking_lot::Mutex<Option<T>>` or `parking_lot::RwLock<Option<T>>` instead

## Platform-Specific Code

Conditional compilation targets for Unix signal handling, macOS Keychain, and cross-platform paths.

- `#[cfg(unix)]` — Process signal handling (SIGUSR1 for restart), nix crate for signal/process
- `#[cfg(target_os = "macos")]` — Keychain integration for credential reading
- Cross-platform path resolution via `dirs` crate

## Error Handling

All IPC commands return `Result<T, String>` for frontend-friendly errors. Internal functions use `.map_err()` chains with context. No panics in public APIs.

`log::error!()` / `log::warn!()` for debugging. Graceful degradation throughout.

## Data Paths

Key filesystem locations used by the backend for storage, config, and caching.

| Path | Platform | Purpose |
|------|----------|---------|
| `~/.local/share/com.quilltoolkit.app/` | Linux | DB, search index, auth secret |
| `~/Library/Application Support/com.quilltoolkit.app/` | macOS | DB, search index, auth secret |
| `~/.config/quill/` | All | Deployed hooks, MCP server, scripts |
| `~/.claude/` | All | Claude Code config, credentials |
| `~/.cache/quill/` | All | Instance state files, restart flags |

### Demo-mode path override

All call-sites that previously hard-coded the data dir, learned-rules dir, or Claude projects dir now route through [[src-tauri/src/data_paths.rs#resolve_data_dir_with_default]], [[src-tauri/src/data_paths.rs#resolve_rules_dir_with_default]], and [[src-tauri/src/data_paths.rs#resolve_claude_projects_dir_with_default]] so a maintainer can launch a sandboxed Quill instance against dummy data without touching their personal state.

The override is gated by an explicit opt-in: `QUILL_DEMO_MODE=1` is required, and `QUILL_DATA_DIR` / `QUILL_RULES_DIR` / `QUILL_CLAUDE_PROJECTS_DIR` are otherwise ignored even when set. With opt-in active and a per-variable override set, paths are canonicalized via `std::fs::canonicalize` (creating the directory first if missing); a canonicalize failure exits the process with code 2 so the demo never silently falls back to the real data dir under a confused launcher. A one-time `[quill-demo] data_dir=… rules_dir=…` banner prints to stderr on first resolver call so a demo run is impossible to confuse with a real one. With `QUILL_DEMO_MODE` unset, behavior is byte-identical to the production path table above.

Used by the marketing-site screenshot-capture workflow (`scripts/run_quill_demo.sh` / `.ps1`); see [[infrastructure#Scripts#Demo Launcher]].
