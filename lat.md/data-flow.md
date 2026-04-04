# Data Flow

The system has five primary data pipelines connecting hook scripts, the HTTP server, the database, and the frontend.

## Token Reporting Pipeline

Hook scripts capture token usage from Claude Code and Codex sessions and report it to the widget for real-time tracking.

1. Claude Code or Codex session produces transcript/state with token counts
2. Provider hook script (`report-tokens.sh`) extracts tokens and POSTs to `POST /api/v1/tokens` with Bearer auth
3. [[src-tauri/src/server.rs]] validates, rate-limits, and inserts into `token_snapshots` table
4. Server emits `tokens-updated` Tauri event
5. Frontend hooks (`useTokenData`, `useAnalyticsData`) receive event and refresh via IPC
6. Hourly cleanup task aggregates snapshots into `token_hourly` by provider/host for historical queries

### Data Shape

`TokenReportPayload` carries provider, session id, hostname, timestamp, token counts, and cwd.

That keeps combined analytics provider-safe while still sharing one token pipeline.

Analytics session drill-down uses the same provider plus session id pair when requesting token history, compact token stats, or session deletion, so identical ids from different providers stay isolated.

## Learning Analysis Pipeline

Tool-use observations and git history are analyzed by LLMs to discover reusable behavioral patterns.

1. Provider hook script (`observe.cjs`) captures PreToolUse/PostToolUse events
2. POSTs observation to `POST /api/v1/learning/observations`
3. Observations stored in `observations` with provider provenance, marked unanalyzed
4. Trigger fires (on-demand, session-end, or periodic timer) with optional provider scope from the UI or session-end payload
5. [[src-tauri/src/learning.rs]] spawns async analysis task scoped to Claude, Codex, or both providers
6. **Stream A**: Fetch up to 100 unanalyzed observations, compress for LLM context
7. **Stream B**: Fetch git history for project via [[src-tauri/src/git_analysis.rs]] (cached by HEAD hash)
8. Haiku extracts patterns from each stream independently
9. Sonnet synthesizes combined findings and applies verdicts on existing rules
10. New rules stored in `learned_rules` with `provider_scope` and written to Claude, Codex, or shared learned-rule directories
11. Existing rule confidence updated using Wilson lower-bound scoring with freshness decay
12. `learning-updated` event emitted; real-time `learning-log` events stream progress to UI

### Observation Compression

Observations are compressed for LLM context using [[src-tauri/src/prompt_utils.rs]]: errors prioritized, then file paths, then outcomes. UTF-8 boundary-aware truncation fits within token budgets.

## Session Indexing Pipeline

Session transcripts are indexed for full-text search with enriched metadata, while provider-aware side tables keep tool and latency data distinct.

1. Claude Code writes session JSONL files to `~/.claude/projects/`, and Codex writes rollout transcripts to `~/.codex/sessions/`
2. On app startup, [[src-tauri/src/sessions.rs]] scans both provider transcript roots incrementally by mtime
3. Provider hook scripts can also post `POST /api/v1/sessions/notify` with JSONL path plus provider metadata
4. Or direct message ingestion via `POST /api/v1/sessions/messages`
5. Provider-specific parsers enrich messages: Claude tool blocks and Codex function/custom tool calls become tools_used, files_modified, code_changes, commands_run, and tool details
6. Indexed into Tantivy with fields: provider, message_id, session_id, content, role, project, host, timestamp, git_branch, plus enriched metadata
7. Tool action details and response-time metrics are stored in provider-aware SQLite tables for deep inspection via MCP and analytics
8. Frontend search queries use TF-IDF weighted scoring with snippet generation
9. Faceted search pre-aggregates provider, project, and host counts

### Enrichment

Each message is enriched during indexing by parsing tool call inputs and outputs.

Claude Edit/Write tool calls become `code_changes`, Bash becomes `commands_run`, and Read/Grep/Glob become `tool_details`. Codex `apply_patch` calls become `code_changes`, `exec_command` and `write_stdin` become `commands_run`, and MCP or auxiliary tool calls become searchable `tool_details`.

## Memory Optimization Pipeline

LLM analyzes project memory files to suggest consolidation, cleanup, and improvements.

1. Frontend triggers optimization for a specific project path plus optional provider scope
2. [[src-tauri/src/memory_optimizer.rs]] scans project memory files plus provider instruction files
3. Filters: exclude denylisted directories, minified/compiled files, oversized content
4. Compute dynamic budget allocation based on available section types
5. Assemble LLM prompt: memory file contents + scoped `CLAUDE.md` or `AGENTS.md` instruction files + learned rules + instinct sections
6. Call Haiku to generate structured optimization suggestions
7. Suggestions stored in `optimization_suggestions` with `provider_scope` and status=pending
8. `memory-optimizer-updated` event notifies frontend
9. User reviews suggestions in the Memories panel with provider badges and a shared provider filter
10. On approve: execute action (write/delete/merge file), store backup in `backup_data` column, set status=executed
11. On deny: set status=denied (can be un-denied later)
12. On undo: restore from backup_data, set status=reverted
13. `memory-files-updated` event triggers UI refresh

### Suggestion Types

Five action types that the LLM can propose for memory files.

- **Delete**: Remove redundant or stale memory files
- **Update**: Rewrite content for clarity or accuracy
- **Merge**: Combine related memory files into one (tracks merge_sources)
- **Create**: Add missing memory documentation
- **Flag**: Mark for human review (no automated action)

## Plugin Management Pipeline

Plugin lifecycle operations through marketplace git repositories and the Claude CLI.

1. Marketplaces registered in `~/.claude/plugins/known_marketplaces.json` (git repos)
2. Each marketplace exposes a plugin manifest
3. [[src-tauri/src/plugins.rs]] enumerates installed from `~/.claude/plugins/installed_plugins.json`
4. Background task checks for updates every 4 hours (lenient semver comparison)
5. `plugin-updates-available` event updates TitleBar badge count
6. Install/update/remove delegate to `claude plugin` CLI subprocess
7. Enable/disable toggle a blocklist and emit `plugin-changed`
8. Bulk updates emit per-plugin `plugin-bulk-progress` events
9. Marketplace refresh: git pull to sync latest manifests

## Usage Bucket Fetching

The main window polls enabled providers for live rate limit status and stores the results in shared provider-aware usage tables.

1. `fetch_usage_data()` resolves the enabled provider list from the integration manager
2. Claude polling in [[src-tauri/src/fetcher.rs]] calls the Anthropic API with an OAuth Bearer token and parses Claude bucket keys
3. Codex polling in [[src-tauri/src/fetcher.rs]] first calls `codex app-server` over stdio and requests `account/rateLimits/read`, which returns a multi-bucket `rateLimitsByLimitId` view that includes the base Codex limits plus model-specific limits such as Codex Spark
4. The fetcher skips unrelated stdio frames like the `initialize` response and only parses the app-server message whose request id matches the rate-limit call
5. Each bucket is normalized to `{ provider, key, label, utilization, resets_at }` and validated for finite utilization plus RFC3339 reset timestamps
5a. Each Codex rate-limit snapshot may also carry a `credits` object (`balance`, `hasCredits`, `unlimited`). The fetcher extracts the first non-null, non-unlimited credit balance and returns it as a `ProviderCredits` entry alongside the buckets
6. If the direct Codex app-server request fails, [[src-tauri/src/fetcher.rs]] falls back to the newest `token_count` transcript event in `~/.codex/sessions/**/*.jsonl` so older Codex installs can still populate base usage rows
7. Successful live buckets are inserted into `usage_snapshots`, keyed by provider plus bucket key, and hourly cleanup aggregates them into `usage_hourly`
8. If a provider poll fails, the command loads the last stored buckets for that provider and returns a provider-scoped error alongside the cached rows
9. Frontend live usage groups rows by provider, while analytics selects one concrete provider bucket for utilization history and stats
