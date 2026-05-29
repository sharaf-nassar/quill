# Data Flow

The system has six primary data pipelines connecting hook scripts, the HTTP server, the database, and the frontend.

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

Hook-reported tokens still flow into `token_snapshots` keyed by the parent `session_id` — Claude sub-agents share the parent's session id on disk, so each row also carries `is_sidechain`/`agent_id`/`parent_uuid` from migration 20. The [[backend#Tauri IPC Commands#Usage and Token Commands (13)]] `get_session_breakdown` rollup aggregates parent and sub-agent rows at query time so a sub-agent's tokens count toward the parent session's totals, and `get_llm_runtime_stats(scope = "parent_only")` is available when the Now-tab card needs to exclude the sub-agent traffic instead.

## Learning Analysis Pipeline

Tool-use observations, git history, and recent session history are analyzed by LLMs to discover reusable behavioral patterns.

1. Provider hook script (`observe.cjs`) captures PreToolUse/PostToolUse events. The Claude script applies a low-signal pre-tool skip list (`Read`, `Glob`, `Grep`, `Bash`, `LS`, `WebSearch`, `WebFetch`, `Agent` — post-phase still records outcomes for those) and a high-signal post-tool allowlist (`Bash`, `Edit`, `Write`, `MultiEdit`, `NotebookEdit`); other post-tool calls — Read/Grep/Glob, `mcp__quill__*`, `mcp__lat__*`, `ToolSearch`, `Skill`, `AskUserQuestion`, etc. — return early because their outcomes carry no behavioral signal for the rule learner. The Codex script captures only `exec_command` (Bash). A 30-day audit showed the unfiltered post-tool firehose contributed ~50% of `observations` rows with zero downstream value
2. POSTs observation to `POST /api/v1/learning/observations`
3. Server validates and fast-acknowledges the hook request, then stores the observation in `observations` with provider provenance and `analyzed = false`
4. Trigger fires from the on-demand UI action or periodic timer with optional provider scope from the UI
5. [[src-tauri/src/learning.rs]] spawns async analysis task scoped to Claude, Codex, or both providers
6. **Stream A**: Fetch up to 100 unanalyzed observations, compress for LLM context
7. **Stream B**: Fetch git history for project via [[src-tauri/src/git_analysis.rs]] (cached by HEAD hash)
8. **Stream C** ([[src-tauri/src/learning.rs#analyze_sessions_stream]]): select recent top-level sessions from Quill's own local session index (cross-project, provider-scoped, recency-capped) and assemble secret-redacted per-session digests via [[src-tauri/src/learning.rs#build_session_digests]] — no external `claude /insights` command
9. Sonnet 4.6 extracts patterns from each of the three streams independently via [[src-tauri/src/cc_client.rs#invoke_typed]], which spawns the `claude` CLI in headless one-shot mode; all streams emit the same `StreamFindings` shape
10. Synthesis decision is uniform over the three streams: 0 with findings → run fails; exactly 1 → its findings used directly (Sonnet skipped); ≥2 → Sonnet synthesizes combined findings and applies verdicts on existing rules, also via [[src-tauri/src/cc_client.rs#invoke_typed]]
11. Per-call metadata (tokens, model, durations, cost, cache stats, stop reason) is captured into `learning_runs.inference_metadata` as a JSON array for every stream including `stream_c`
12. New rules stored in `learned_rules` with `provider_scope` and written to Claude, Codex, or shared learned-rule directories
13. Existing rule confidence updated using Wilson lower-bound scoring with freshness decay
14. `learning-updated` event emitted; real-time `learning-log` events stream progress to UI

### Observation Compression

Observations are compressed for LLM context using [[src-tauri/src/prompt_utils.rs]]: errors prioritized, then file paths, then outcomes. UTF-8 boundary-aware truncation fits within token budgets.

## Hook Telemetry Pipeline

Lifecycle-hook fires are surfaced in the Now-tab Hooks breakdown via two distinct paths because Claude and Codex log hooks differently.

1. **Claude path (transcript-derived).** Claude Code writes a `type:"attachment"` JSONL record for every hook invocation, carrying `hookEvent`, `hookName`, `command`, `exitCode`, `durationMs`, and `parentUuid`. The existing [[backend#Session Indexing]] sweep already walks those JSONL files; [[src-tauri/src/sessions.rs#extract_hook_invocation_from_attachment]] peels off the hook records in the same parse pass that produces messages and `session_events`, canonicalizes the script command via [[src-tauri/src/sessions.rs#canonicalize_hook_identity]] (`quill:<basename>` / verbatim `${CLAUDE_PLUGIN_ROOT}/…` / basename / `hookName` fallback), and the indexing loop calls [[src-tauri/src/storage.rs#Storage#store_hook_invocations_for_messages]] on a per-batch transaction with `INSERT OR IGNORE` against the UNIQUE identity index. Sub-agent transcripts feed the same path with `is_sidechain=1` and `agent_id` preserved.
2. **Codex path (live observer).** Codex rollouts do not record hook executions, so the installer registers `src-tauri/codex-integration/scripts/hook-observe.cjs` on every one of the eight Codex hook events when `activity_tracking` is enabled. On each fire the script reads stdin, builds a `{provider, session_id, hook_event, tool_name, cwd, ts, hook_matcher}` payload, and POSTs to `POST /api/v1/hooks/observed` with bearer auth and a 1.5-second local timeout.
3. The HTTP handler in [[src-tauri/src/server.rs]] validates the eight-event whitelist plus length caps and ISO-8601 timestamp shape, fast-acks `202 Accepted`, and dispatches the insert via [[src-tauri/src/storage.rs#Storage#store_codex_hook_observation]] on a `tokio::task::spawn_blocking`. Codex identity is event-scoped (`hook_event` plus optional `:tool_name`) because the observer fires per-event, not per-script.
4. The server emits `hooks-observed-updated` after a successful Codex insert. [[src/hooks/useBreakdownData.ts#useBreakdownData]] subscribes to that channel (alongside `tokens-updated` and `sessions-index-updated`) with a 1-second debounce so Codex live fires tick the Hooks breakdown within ~2 seconds without flooding the IPC layer.
5. Migration 27 seeds `hook_invocation_reingest_pending`; the post-boot sweep in [[src-tauri/src/sessions.rs]] picks that flag up alongside the existing skill/runtime reingest flags and replays the attachment extractor across every Claude transcript, then clears the flag only after the sweep completes cleanly. Codex has no historical backfill — its rows accrue prospectively via the observer endpoint.
6. The frontend [[src/components/analytics/BreakdownPanel.tsx]] renders one row per `hook_identity`, keeps `quill:` prefixes visible in the identity text, and includes a help affordance that explains the Claude/Codex tracking asymmetry.

The privacy gate is the existing `activity_tracking` feature flag: toggling it off removes `hook-observe.cjs` and its eight `[[hooks.<Event>]]` config entries from `~/.codex/config.toml`, stopping new Codex observations. Claude-side ingestion is unaffected because Quill never writes anything to Claude transcripts — the data flows from the user's transcript to Quill, not the other way around.

## Session Indexing Pipeline

Session transcripts are indexed for full-text search with enriched metadata, while provider-aware side tables keep tool and latency data distinct.

1. Claude Code writes session JSONL files to `~/.claude/projects/`, and Codex writes rollout transcripts to `~/.codex/sessions/`
2. When Session Search opens, [[src-tauri/src/sessions.rs]] scans both provider transcript roots incrementally by mtime
3. Provider hook scripts can also post `POST /api/v1/sessions/notify` with JSONL path plus provider metadata, while incremental remote sync can push `POST /api/v1/sessions/messages`
4. `notify` requests are acknowledged first, then coalesced per session and replayed as one atomic delete-and-rebuild batch; `messages` batches append under one writer lock and commit atomically
5. Local Claude full-transcript sync now runs on Stop instead of every PostToolUse event, so full-file reindexing happens at a stable boundary instead of after each tool completion
6. Provider-specific parsers enrich messages: Claude tool blocks and Codex function/custom tool calls become tools_used, files_modified, code_changes, commands_run, and tool details
7. Indexed into Tantivy with fields: provider, message_id, session_id, content, role, project, host, timestamp, git_branch, plus enriched metadata
8. Tool action details and response-time metrics are stored in provider-aware SQLite tables for deep inspection via MCP and analytics, with transcript reindexing batching tool-action inserts per file/session
9. Frontend search queries use TF-IDF weighted scoring with snippet generation
10. Faceted search pre-aggregates provider, project, and host counts

### Enrichment

Each message is enriched during indexing by parsing tool call inputs and outputs.

Claude Edit/Write tool calls become `code_changes`, Bash becomes `commands_run`, and Read/Grep/Glob become `tool_details`. Codex `apply_patch` calls become `code_changes`, `exec_command` and `write_stdin` become `commands_run`, and MCP or auxiliary tool calls become searchable `tool_details`.

### Dual Emission for Runtime Tracking

The same parse pass that produces `ExtractedMessage` for the search index also produces `ExtractedEvent` for the [[backend#Database#Schema#Code and Runtime Metrics]] `session_events` table.

The search index keeps its existing filter (it drops `tool_result`-only user messages and empty assistant blocks), while the event stream carries every non-meta `user`/`assistant` line with a non-empty timestamp — classified by content shape into `user_text`, `user_tool_result`, `asst_text`, `asst_thinking`, or `asst_tool_use`. This dual emission keeps the search corpus lean while letting the runtime metric reconstruct full agent-loop active intervals. Both the local mtime sweep in [[src-tauri/src/sessions.rs]] and the hook-driven `/sessions/notify` handler in [[src-tauri/src/server.rs]] ingest into `session_events` alongside `response_times`; the `/sessions/messages` remote-push handler uses a heuristic classifier over `(role, content, tools_used)` because wire-pushed messages lack the full content-block shape that the JSONL extractor sees.

Feature 009 adds a third sibling to this dual emission: the same Claude JSONL walk peels off `type:"attachment"` records whose `attachment.type` begins `hook_` (e.g. `hook_success`, `hook_failure`, `hook_timeout`, `hook_blocked`) via [[src-tauri/src/sessions.rs#extract_hook_invocation_from_attachment]], producing one [[backend#Database#Schema#Hook Invocations]] row per fire. Sub-agent transcripts inherit the `is_sidechain=1` and `agent_id` columns automatically because the attachment extractor reads the same record-level fields the message extractor does. Codex rollouts do not emit attachment records, so Codex hook telemetry instead arrives live via `POST /api/v1/hooks/observed` from a deployed observer script.

### Sub-Agent Transcripts

The Claude file walker now picks up `<projectSlug>/<session-uuid>/subagents/agent-*.jsonl` in addition to the flat parent transcript so sub-agent activity flows through the same enrichment and storage path.

Each sub-agent file becomes a separate ingest entry, but rows write the parent's `session_id` (matching the on-disk `sessionId` field) plus `is_sidechain=1` and the sub-agent's `agent_id`, while parent-transcript rows stay `is_sidechain=0`. Codex emits no sub-agent transcripts today, so its ingest path writes the same defaults (`is_sidechain=0`, `agent_id=NULL`, `parent_uuid=NULL`) and inherits the rollup behavior whenever the OpenAI CLI gains a sub-agent feature.

## Memory Optimization Pipeline

LLM analyzes project memory files to suggest consolidation, cleanup, and improvements.

1. Frontend triggers optimization for a specific project path plus optional provider scope
2. [[src-tauri/src/memory_optimizer.rs]] scans project memory files plus provider instruction files
3. Filters: exclude denylisted directories, minified/compiled files, oversized content
4. Compute dynamic budget allocation based on available section types
5. Assemble LLM prompt: memory file contents + scoped `CLAUDE.md` or `AGENTS.md` instruction files + learned rules + instinct sections
6. Call Sonnet 4.6 via [[src-tauri/src/cc_client.rs#invoke_typed]] (`claude` CLI headless mode) to generate structured optimization suggestions; per-call metadata is captured into `optimization_runs.inference_metadata`
7. Backend validates suggestion shape before storage: malformed merges, missing content/targets, instruction-file merges, and other unsafe outputs are discarded instead of being surfaced in the UI
8. Valid suggestions stored in `optimization_suggestions` with `provider_scope` and status=pending
9. `memory-optimizer-updated` event notifies frontend
10. User reviews suggestions in the Memories panel with provider badges and a shared provider filter
11. On approve: execute action (write/delete/merge file), store backup in `backup_data` column, set status=executed
12. On deny: set status=denied (can be un-denied later)
13. On undo: restore from backup_data, set status=reverted
14. `memory-files-updated` event triggers UI refresh

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
2a. Before Claude makes a live request, [[src-tauri/src/lib.rs]] reuses the most recent persisted `usage_snapshots` rows when they are newer than the 3-minute live refresh interval, so window reopens and app restarts do not immediately hit the Anthropic endpoint again
3. Codex polling in [[src-tauri/src/fetcher.rs]] first calls `codex app-server` over stdio and requests `account/rateLimits/read`, which returns a multi-bucket `rateLimitsByLimitId` view that includes the base Codex limits plus model-specific limits such as Codex Spark
4. The fetcher skips unrelated stdio frames like the `initialize` response and only parses the app-server message whose request id matches the rate-limit call
5. Each bucket is normalized to `{ provider, key, label, utilization, resets_at }` and validated for finite utilization plus RFC3339 reset timestamps
5a. Each Codex rate-limit snapshot may also carry a `credits` object (`balance`, `hasCredits`, `unlimited`). The fetcher extracts the first non-null, non-unlimited credit balance and returns it as a `ProviderCredits` entry alongside the buckets
6. If the direct Codex app-server request fails, [[src-tauri/src/fetcher.rs]] falls back to the newest `token_count` transcript event in `~/.codex/sessions/**/*.jsonl` so older Codex installs can still populate base usage rows
7. Successful live buckets are inserted into `usage_snapshots`, keyed by provider plus bucket key, and hourly cleanup aggregates them into `usage_hourly`
8. If a provider poll fails, the command loads the last stored buckets for that provider and returns a provider-scoped error alongside the cached rows
8a. Claude 429 responses persist a silent cooldown timestamp in the settings store, and subsequent refreshes honor that cooldown before retrying the live API. These rate limits are not returned as provider errors. A 401 from the usage API is treated as a stale access token, not a logout: [[src-tauri/src/fetcher.rs#fetch_claude_usage]] returns a `Paused` kind with a neutral message, the poll pushes `ProviderErrorKind::Paused` via [[src-tauri/src/lib.rs#push_paused_error]], and [[src/components/UsageDisplay.tsx]] shows a muted "Paused" badge with any cached rows still rendered (and the badge alone on a first-run empty view) — never a login prompt or red error. To keep that guarantee, [[src-tauri/src/lib.rs#build_usage_data]] excludes `Paused` when picking the top-level `error`, so a stale-token poll with no cached rows yet never surfaces a red "Failed to load usage data". The red "Run: claude /login" guidance is reserved for a confirmed logout: no local credentials AND `claude auth status` reporting `loggedIn: false` (see 8d).
8b. Transport failures (DNS, connect refused, pre-response timeout) on Claude or MiniMax persist a per-provider network cooldown computed by [[src-tauri/src/lib.rs#compute_network_backoff]] — half-jitter exponential with a 60-second base, 30-minute cap, doubled per consecutive failure. The cooldown lives in the backend; the frontend `setInterval` poller keeps firing every 3 minutes but each call is short-circuited inside `refresh_usage_cache` and returns cached rows without a live HTTP request. The backend `tokio` loop hits the same short-circuit. No live request is made for either polling path during the cooldown. The poll pushes a typed `ProviderErrorKind::Network` so [[src/components/UsageDisplay.tsx]] can render a single consolidated "Offline — showing cached data" pill instead of one red banner per provider. On any successful fetch, both cooldown timestamps and the consecutive-failure counter clear. The fast offline signal itself comes from [[src-tauri/src/config.rs#http_client]]'s 5-second connect timeout (15-second overall), so reqwest never hangs on a dead network.
8c. The kind classification originates in the fetcher: [[src-tauri/src/fetcher.rs#ClaudeUsageError]] exposes a Claude `kind` (`Credentials`/`Paused`/`RateLimited`/`Request`/`Api`/`Parse`) and [[src-tauri/src/fetcher.rs#MiniMaxUsageError]] exposes its own (`Unauthorized`/`RateLimited`/`Request`/`Api`/`Parse`). The polling layer in [[src-tauri/src/lib.rs]] maps Claude `Request` to `ProviderErrorKind::Network` (driving the cooldown), `RateLimited` to a silent rate-limit cooldown, `Paused` (401, stale token) to the muted `ProviderErrorKind::Paused`, `Credentials` (no local token) to `Config` only after the logout confirmation in 8d (otherwise `Paused`), and the remaining variants to `Server`. MiniMax still maps `Unauthorized` to `Auth`. The mapping itself lives in the pure helpers [[src-tauri/src/lib.rs#classify_claude_error_kind]] and [[src-tauri/src/lib.rs#classify_minimax_error_kind]] so the match can be unit-tested without touching storage. Cooldown bookkeeping (skip-on-active, write-on-error, clear-on-success) goes through the per-provider [[src-tauri/src/lib.rs#ProviderCooldownKeys]] struct: each provider declares a constant value of that struct (`CLAUDE_COOLDOWN_KEYS`, `MINIMAX_COOLDOWN_KEYS`) wiring its four setting keys to the shared helpers [[src-tauri/src/lib.rs#check_provider_cooldown]], [[src-tauri/src/lib.rs#clear_provider_cooldowns]], [[src-tauri/src/lib.rs#write_rate_limit_cooldown]], and [[src-tauri/src/lib.rs#record_network_failure]]. Adding a new provider is a typed `<Provider>UsageError` in `fetcher.rs`, a fifth setting-key quartet, a constant `<PROVIDER>_COOLDOWN_KEYS` value, and a `classify_<provider>_error_kind` mapping — no further branching needed.
8d. When a Claude poll yields the `Credentials` kind (no local access token), the poller confirms the logout before warning. [[src-tauri/src/lib.rs#resolve_claude_logout_or_paused]] calls [[src-tauri/src/config.rs#claude_logged_in]], which spawns `claude auth status --json` UNCONFINED — a plain `tokio::process::Command` with the inherited environment and a ~15s timeout, NO Landlock/bwrap/sandbox-exec, NO prompt, NO `-p`, NO inference, and NO write to the credential store. Only `loggedIn: false` produces the red `Config` (logged-out) error; `loggedIn: true` or any inconclusive failure (binary missing, spawn error, timeout, parse failure) downgrades to `Paused` with cached rows and no warning. The verdict is cached for ~120s (`CLAUDE_AUTH_STATUS_CHECKED_AT_KEY` timestamp plus a `CLAUDE_AUTH_STATUS_LOGGED_IN_KEY` boolean) so the 3-minute poller spawns the CLI at most once per TTL; a successful live fetch clears the cache so a fresh login is recognized immediately.
9. Frontend live usage groups rows by provider, while analytics selects one concrete provider bucket for utilization history and stats
10. `emit_usage_updates()` rebuilds the backend-owned indicator state, emits `indicator-updated`, and lets the tray listener update title text plus `Now`, `Resets`, and `Week` summary rows from the same payload
## Indicator Preference Pipeline

The status indicator has one backend-owned provider preference shared across the tray and the [[features#Settings Window]]'s Integrations tab.

1. `useIntegrations()` loads `get_indicator_primary_provider` alongside provider statuses so the Integrations tab starts from the persisted preference
2. The Integrations tab renders an `Auto` option plus enabled providers, and preserves a disabled unavailable option when a saved provider is temporarily missing
3. Changing the selector invokes `set_indicator_primary_provider`, which stores the configured provider in the settings table
4. The backend recomputes `StatusIndicatorState`, emits `indicator-updated`, and updates the tray summary from that backend-owned payload
5. `useIntegrations()` listens for `indicator-updated` to keep all mounted selector instances synchronized
