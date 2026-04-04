# Backend

The Rust backend handles storage, ingestion, search, LLM analysis, plugin management, and provider lifecycle management.

It communicates with the frontend through 68 Tauri IPC commands and 11 documented push events.

## Entry Point

[[src-tauri/src/lib.rs]] (1,132 lines) is the application entry point. It initializes storage, starts the HTTP server, registers all Tauri commands, sets up the tray icon, and launches [[architecture#Background Tasks]].

Tauri plugins configured: `tauri-plugin-log`, `tauri-plugin-updater`, `tauri-plugin-process`, `tauri-plugin-window-state`.

## HTTP API Server

[[src-tauri/src/server.rs]] (868 lines) runs an Axum HTTP server on port 19876 (configurable via `QUILL_PORT` env var) to receive data from external hook scripts.

### Authentication

All endpoints require a Bearer token validated with constant-time comparison (`subtle` crate). The token is generated on first launch by [[src-tauri/src/auth.rs]] and stored at `~/.local/share/com.quilltoolkit.app/auth_secret` with mode 0o600.

### Rate Limiting

Sliding window rate limiter with 60-second buckets. Limits per endpoint type:

| Category | Limit |
|----------|-------|
| General | 100 req/min |
| Observations | 500 req/min |
| Session notify | 500 req/min |
| Session messages | 100 req/min |

### Endpoints

The HTTP API exposes 14 endpoints for token ingestion, learning observations, and session indexing across Claude Code and Codex.

| Method | Route | Purpose |
|--------|-------|---------|
| GET | `/api/v1/health` | Health check |
| POST | `/api/v1/tokens` | Record token usage from hook scripts |
| POST | `/api/v1/learning/observations` | Store tool-use observations |
| GET | `/api/v1/learning/observations` | Retrieve unanalyzed observations |
| POST | `/api/v1/learning/session-end` | Notify that a session ended |
| GET | `/api/v1/learning/status` | Learning system status |
| POST | `/api/v1/learning/runs` | Record a learning analysis run |
| GET | `/api/v1/learning/runs` | Retrieve learning run history |
| POST | `/api/v1/learning/rules` | Store discovered behavioral rules |
| POST | `/api/v1/sessions/notify` | Notify of new session JSONL file |
| POST | `/api/v1/sessions/messages` | Ingest session messages for indexing |
| GET | `/api/v1/sessions/search` | Full-text search sessions |
| GET | `/api/v1/sessions/context` | Get surrounding messages |
| GET | `/api/v1/sessions/facets` | Get search facets |

Each endpoint validates input (length limits, range checks, type validation) before processing. Provider-aware payloads default legacy callers to `claude`, while new Claude and Codex hooks send explicit provider tags for telemetry and session ingestion.

## Database

[[src-tauri/src/storage.rs]] (3,393 lines) manages a SQLite database with WAL mode and 5-second busy timeout. The largest backend module.

### Location

The SQLite database file path varies by operating system.

- Linux: `~/.local/share/com.quilltoolkit.app/usage.db`
- macOS: `~/Library/Application Support/com.quilltoolkit.app/usage.db`

### Schema

The database has 16 tables across 14 migration versions.

#### Usage Tracking

Tables for recording and aggregating provider-aware live usage bucket utilization over time.

- **usage_snapshots** — Raw live usage snapshots keyed by provider plus bucket key (timestamp, provider, bucket_key, bucket_label, utilization, resets_at).
- **usage_hourly** — Hourly aggregates keyed by provider plus bucket key (avg/max/min utilization, sample_count). Unique on (hour, provider, bucket_key).

Live usage ingestion stores Claude API buckets and Codex transcript-derived rate-limit buckets in the same tables. Codex `rate_limits.resets_at` values are normalized from transcript epoch timestamps into RFC3339 strings before storage so the live pane can show the same reset countdown semantics as Claude. Migration 14 backfills older Claude-only rows by deriving stable bucket keys from legacy labels, and startup creates provider-only indexes after migrations so older databases can still boot before those columns exist.

#### Token Tracking

Tables for recording per-session token consumption and hourly host-level aggregates with provider provenance.

- **token_snapshots** — Raw token usage per provider/session (provider, session_id, hostname, timestamp, input/output/cache tokens, cwd). Indexed on provider-aware timestamp, session, and cwd paths.
- **token_hourly** — Hourly aggregates per provider/host (total tokens, turn_count). Unique on (hour, hostname, provider).
- Analytics session history, compact token stats, and delete-session cleanup all treat sessions as `(provider, session_id)` pairs so Claude and Codex ids cannot collide.

#### Learning System

Tables for the behavioral learning pipeline: observations, summaries, analysis runs, and discovered rules.

- **observations** — Tool-use observations (provider, session_id, hook_phase, tool_name, tool_input/output, cwd). Indexed on session_id, timestamp, created_at, and provider cleanup paths.
- **observation_summaries** — Per-period/provider/project summaries (tool_counts JSON, error_count, total). Unique on (period, provider, project).
- **learning_runs** — Analysis run records (trigger_mode, observations_analyzed, rules created/updated, duration, status, error).
- **learned_rules** — Discovered patterns (name unique, domain, confidence, observation_count, file_path, content, state, is_anti_pattern, source). The `content` column (migration 11) stores sanitized rule text for manual promotion.

#### Session Indexing

Stores detailed tool invocation and response-time data for MCP-powered session search.

- **tool_actions** — Tool invocation details for MCP (provider, message_id, session_id, tool_name, category, file_path, summary, full_input/output). Indexed on provider/session, message_id, file_path, and category.
- **response_times** — Assistant response latency per provider/session turn (provider, session_id, timestamp, response_secs, idle_secs). Unique on (provider, session_id, timestamp).

#### Memory Optimizer

Tables for tracking memory files, optimization runs, and actionable suggestions with lifecycle management.

- **memory_files** — Tracked memory files (project_path, file_path, content_hash, last_scanned_at). Unique on (project_path, file_path).
- **optimization_runs** — Optimization run records (project_path, trigger, memories_scanned, suggestions_created, status, timestamps).
- **optimization_suggestions** — Suggestions with lifecycle (run_id FK, action_type, target_file, reasoning, proposed_content, status, backup_data, group_id). Indexed on run_id, project_path+status, group_id.

#### Code and Response Metrics

Tables for tracking response latency per turn and caching git commit history per project.

- **git_snapshots** — Cached git history per project (project unique, commit_hash, commit_count, raw_data).

#### Metadata

Key-value configuration and schema migration version tracking.

- **settings** — Key-value config storage.
- **schema_version** — Migration version tracking (currently v14).

## Tauri IPC Commands

68 async commands registered in [[src-tauri/src/lib.rs]], grouped by feature.

### Usage and Token Commands (10)

`fetch_usage_data`, `get_usage_history`, `get_usage_stats`, `get_all_bucket_stats`, `get_snapshot_count`, `get_token_history`, `get_token_stats`, `get_token_hostnames`, `get_host_breakdown`, `get_session_breakdown`.

The live-usage commands now treat utilization history as `(provider, bucket_key)` data instead of assuming a single global Claude bucket label.

Codex live usage now comes from `codex app-server` `account/rateLimits/read` instead of transcript-only scraping. The backend normalizes the returned `rateLimitsByLimitId` map into provider buckets so Quill can store both the base Codex windows and model-specific limits such as Codex Spark in the same usage tables, while preserving the legacy base Codex bucket keys for history continuity. Model-specific `limitName` values are abbreviated for display via [[src-tauri/src/fetcher.rs#abbreviate_codex_model]] (e.g. `GPT-5.3-Codex-Spark` → `5.3-Spark`) by stripping the redundant `GPT-` prefix and `-Codex` infix. The stdio helper ignores unrelated app-server frames such as the `initialize` response and only deserializes the matching request id for the rate-limit call. If the direct app-server request fails, the fetcher falls back to transcript `token_count` `rate_limits`.

`get_session_breakdown` now accepts optional provider and limit arguments so Codex live views can request a provider-scoped active set without being crowded out by Claude sessions.

### Project and Session Management (7)

`get_project_tokens`, `get_session_stats`, `get_project_breakdown`, `delete_project_data`, `rename_project`, `delete_host_data`, `delete_session_data`.

### Integration Commands (3)

Commands for detecting supported CLI providers and running provider-specific install and uninstall flows.
Provider setup state is persisted through the settings table using key `integration.providers.v1`
to survive app restarts.

`get_provider_statuses`, `confirm_enable_provider`, `confirm_disable_provider`.

### Learning Commands (12)

Commands for managing the behavioral learning pipeline settings, rules, and observations.
Read and trigger commands accept an optional provider filter so the UI can request Claude-only, Codex-only, or combined learning views.

`get_learning_settings`, `set_learning_settings`, `get_learned_rules`, `delete_learned_rule`, `promote_learned_rule`, `get_learning_runs`, `trigger_analysis`, `get_observation_count`, `get_unanalyzed_observation_count`, `get_top_tools`, `get_observation_sparkline`, `read_rule_content`.

### Code and Response Stats (4)

`get_code_stats`, `get_code_stats_history`, `get_batch_session_code_stats`, `get_response_time_stats`.

### Memory Optimizer Commands (16)

Commands for managing memory files, optimization runs, and suggestion approval workflows.
Most read and trigger commands accept an optional provider filter for Claude, Codex, or combined views.

`get_memory_files`, `trigger_memory_optimization`, `get_optimization_suggestions`, `approve_suggestion`, `deny_suggestion`, `undeny_suggestion`, `undo_suggestion`, `approve_suggestion_group`, `deny_suggestion_group`, `get_suggestions_for_run`, `get_optimization_runs`, `get_known_projects`, `add_custom_project`, `remove_custom_project`, `delete_memory_file`, `delete_project_memories`.

### Plugin Commands (14)

Commands for installing, updating, enabling, and managing plugins and marketplaces.
All plugin commands take a provider argument so the frontend can target Claude or Codex explicitly while keeping one shared window.

`get_installed_plugins`, `get_marketplaces`, `get_available_updates`, `check_updates_now`, `install_plugin`, `remove_plugin`, `enable_plugin`, `disable_plugin`, `update_plugin`, `update_all_plugins`, `add_marketplace`, `remove_marketplace`, `refresh_marketplace`, `refresh_all_marketplaces`.

Claude plugin mutations delegate to the `claude plugin` CLI and marketplace git repos. Codex plugin reads and install/remove operations use `codex app-server` JSON-RPC over stdio, while unsupported Codex mutations return provider-specific errors instead of guessing behavior.

### Session Indexing Commands (4)

`search_sessions`, `get_session_context`, `get_search_facets`, and `rebuild_search_index` all operate on a unified Claude-plus-Codex index. Search and context requests include provider identity so session collisions do not bleed across providers.

### Restart Commands (7)

`discover_restart_instances`, `discover_claude_instances` (compat alias), `request_restart`, `cancel_restart`, `get_restart_status`, `install_restart_hooks`, `check_restart_hooks_installed`.

Restart commands expose a shared provider-aware row model across Claude and Codex. Hook install/check commands accept an optional provider parameter so restart setup can be applied per provider.

### UI Commands (2)

`hide_window`, `quit_app`.

### Integration Commands (2)

`get_provider_statuses` and `confirm_enable_provider` expose provider detection and enable state.
Detection runs via `--version` checks and profile directory presence checks for CLI providers.
Implementation lives in [[src-tauri/src/integrations/mod.rs]].

## Event System

The backend pushes real-time updates to the frontend via Tauri's emit system.

| Event | Source | Payload | Trigger |
|-------|--------|---------|---------|
| `tokens-updated` | server.rs | `()` | Token snapshot stored |
| `learning-log` | learning.rs | `{run_id, message}` | Real-time analysis progress |
| `learning-updated` | lib.rs | `()` | Rules changed |
| `plugin-changed` | lib.rs | `()` | Plugin or marketplace mutation completed |
| `plugin-bulk-progress` | plugins.rs | `BulkUpdateProgress` | Per-plugin update progress |
| `plugin-updates-available` | plugins.rs | `count` | New updates found |
| `provider-status-updated` | integrations | `Vec<ProviderStatus>` | Startup provider detection refresh |
| `restart-status-changed` | restart.rs | `RestartStatus` | Restart phase change |
| `integrations-updated` | integrations/manager.rs | `ProviderStatus[]` | Startup refresh or provider enable/disable completed |
| `memory-optimizer-log` | memory_optimizer.rs | `{message}` | Optimization run progress |
| `memory-optimizer-updated` | memory_optimizer.rs | `{run_id, status}` | Run completed |
| `memory-files-updated` | memory_optimizer.rs | `{project_path}` | Memory files changed |

## Session Indexing

[[src-tauri/src/sessions.rs]] provides full-text search over Claude Code and Codex transcripts using Tantivy, with provider-safe identity for indexing, search hits, context lookup, and reindex cleanup.

### Index Schema

The Tantivy index stores provider, identity, content, and enrichment fields for shared session search.

Fields include provider, message_id, session_id, content, role, project, host, timestamp, git_branch, tools_used, files_modified, code_changes, commands_run, tool_details, and a stored display text field. Provider/project/host are faceted for filters. Stored at `~/.local/share/com.quilltoolkit.app/session-index/`.

### Indexing Strategy

Startup indexing scans `~/.claude/projects/` and `~/.codex/sessions/**` incrementally by mtime.

The HTTP API also accepts provider-tagged notify and direct message ingestion. TF-IDF scoring plus snippet generation power the shared search UI with provider filters and badges.

## AI Client

[[src-tauri/src/ai_client.rs]] (118 lines) wraps the Anthropic API via rig-core SDK.

Uses OAuth Bearer token authentication (`sk-ant-oat01-...` prefix). Model routing: Haiku for pattern extraction (fast, cheap), Sonnet for synthesis (deeper reasoning). Supports generic typed analysis with any `JsonSchema`-compatible output type via `schemars`.

## Git Analysis

[[src-tauri/src/git_analysis.rs]] (343 lines) extracts commit patterns for the [[features#Learning System]].

Collects commit messages, file hotspots (change frequency), co-change patterns (files changed together), and directory structure. Excludes merge commits (>20 files) and minified code. Results cached by project + HEAD commit hash, invalidated on HEAD change. Compressed to 4,500 bytes for LLM context.

## Concurrency

The backend uses Tokio for async operations with specific patterns:

- `tokio::task::block_in_place()` for sync DB/file operations within async context
- `tokio::spawn()` and `tauri::async_runtime::spawn()` for background tasks
- `parking_lot::Mutex<T>` for fast synchronization (preferred over `std::sync::Mutex`)
- `Arc<T>` for shared ownership across task boundaries
- `OnceLock<T>` for one-time initialization of globals (STORAGE, HTTP_CLIENT)

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
