# Backend

The Rust backend handles storage, ingestion, search, LLM analysis, plugin management, provider lifecycle management, and the cross-platform status indicator.

It communicates with the frontend through a broad Tauri IPC surface and documented push events.

## Entry Point

[[src-tauri/src/lib.rs]] is the application entry point. It initializes storage, starts the HTTP server, registers all Tauri commands, sets up the tray icon, and launches [[architecture#Background Tasks]].

Tauri plugins configured: `tauri-plugin-log`, `tauri-plugin-updater`, `tauri-plugin-process`, `tauri-plugin-window-state`. Session transcript catch-up is no longer part of app launch; the Sessions window requests an incremental sync when search is opened.

[[src-tauri/src/lib.rs#initialize_storage_or_report_fatal]] publishes the process-wide storage handle or returns `None`, and the setup path abandons the rest of startup rather than running against absent storage. It never calls `process::exit` itself: a bare exit made a failed migration look like an app that silently refuses to launch. [[src-tauri/src/lib.rs#report_fatal_storage_failure]] instead hides (not closes — a close on the last window requests app exit and would race the dialog away) every window and queues a `tauri-plugin-dialog` error dialog naming the failure and the database folder, exiting from the dialog callback. The dialog cannot be shown synchronously because setup runs inside the event loop's `Ready` handler and `blocking_show` would freeze the thread it needs. Because termination then hangs off a callback, a session with no working dialog backend would leave a hidden UI-less process alive forever, so a watchdog armed alongside the dialog exits after `FATAL_STORAGE_DIALOG_TIMEOUT` (60s) regardless. Callback and watchdog race for one function-scoped `AtomicBool`, so exactly one terminates and the loser is a no-op.

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

`/api/v1/hooks/observed` (feature 009) shares the **Observations** bucket because both endpoints accept hook-fire telemetry whose call rate scales with tool-call volume in active sessions, and a hook chain that fires `PreToolUse` + `PostToolUse` + Quill's own scripts can saturate a stricter limit on a heavy bash-driven turn. The handler runs `check_auth` → `check_rate_limit_with_max(obs_rate_limiter, MAX_OBS_REQUESTS)` → validation (ten-event whitelist, ISO-8601 timestamp parse, length caps on `tool_name`/`hook_matcher`/`agent_id`) → background insert before returning `202 Accepted`, preserving the fast-ack contract observed by `src-tauri/codex-integration/scripts/hook-observe.cjs`.

### Endpoints

The HTTP API exposes 15 endpoints for token ingestion, context savings, learning observations, session indexing, and hook telemetry across Claude Code and Codex.

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
| POST | `/api/v1/hooks/observed` | Record observed lifecycle-hook fires from Codex (Claude side reads from transcripts) |

Each endpoint validates input (length limits, range checks, type validation) before processing. Provider-aware payloads default legacy callers to `claude`, while new Claude and Codex hooks send explicit provider tags for telemetry and session ingestion. Hook-facing observation and session-ingest POSTs acknowledge after validation and finish SQLite/Tantivy work on background blocking tasks so CLI hooks do not wait on local indexing. Local hook scripts treat receipt of response headers as the success boundary and use a short 1.5-second local timeout, which keeps the CLI path tolerant of slow response teardown without waiting on background indexing.

## Database

[[src-tauri/src/storage.rs]] manages a SQLite database with WAL mode and 5-second busy timeout. The largest backend module.

Most operations use one mutex-protected primary connection. The paired Models aggregate and history reads instead open independent read-only, query-only connections with the same timeout, allowing WAL concurrency while each response retains its own deferred snapshot.

### Location

The SQLite database file path varies by operating system.

- Linux: `~/.local/share/com.quilltoolkit.app/usage.db`
- macOS: `~/Library/Application Support/com.quilltoolkit.app/usage.db`

### Schema

The database schema is versioned through migration 32 and includes usage, token, model analytics, context savings, learning, rule governance, session indexing, memory optimizer, code, runtime, and metadata tables.

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

#### Model Analytics Evidence

Migration 28 stores replayable transcript evidence and source ownership for provider-qualified model analytics without a model catalog.

The source lifecycle and graph-resolution path is documented in [[data-flow#Model Observation Reconciliation]].

- **model_usage_observations** — Normalized turn and token facts with exact raw model identity, a nullable indexed `derived_model_id` attribution column, nullable token dimensions, resolved session/chain ownership, and source-local ordering.
- **model_observation_sources** — Retained source inventory, fingerprints, activity bounds, reconciliation status, and durable deletion suppression.
- **model_backfill_state** — Singleton progress and completeness state used to distinguish final empty claims from provisional recovered data.

Backfill lifecycle writes are transactional and state-guarded. Interrupted and explicit retry initialization advance the inventory generation, clear only run-local counters, and preserve evidence; pending work alone can become running. Root outcomes precede an explicit source-total publication marker, which distinguishes an authoritative empty inventory from work not yet inventoried. Batch counters cannot exceed remaining work, and only a failure-free resolved inventory with at least one configured root and a published source total can finish complete. Partial and failed states persist inventory completeness independently, so unreadable sources and unreadable roots remain distinguishable. Persisted diagnostics use the bounded `ModelBackfillDiagnostic` value rather than raw filesystem errors.

[[src-tauri/src/model_usage.rs#run_retained_model_history_backfill]] owns one retained-history pass under the shared process permit. It inventories each provider root off the async executor and commits its cumulative outcome before starting the next, then prepares stable generation-owned work before publishing the plan's validated source total. Bounded source batches commit and record progress between yields, source failures retain last-good rows, and only completed root proofs prune child rows before parents. Terminal `partial` versus `failed` reflects useful committed work, while inventory completeness depends only on resolved roots and attempted discovered sources.

[[src-tauri/src/lib.rs#run]] resets an interrupted running pass to a fresh `startup_resume` generation after storage initialization and reserves one nonblocking worker for migration-pending or resumed work. The reservation waits for live reconciliation to release the shared process permit instead of discarding historical work.

[[src-tauri/src/storage.rs#Storage#get_model_analytics]] reads aggregates and backfill state from one SQLite snapshot. Global sessions use all unsuppressed retained source ownership, while range sessions, represented providers, evidence, coverage, and model rows require actual normalized timestamps in the half-open range. Cache-read share remains unavailable when any contributing token row lacks a required dimension.

Migration 29 adds the `derived_model_id` column plus its index and re-arms `model_backfill_state` to pending under a bumped generation with a `migration` trigger, so existing installs re-attribute retained evidence on the next startup pass. Because attribution is computed only during parsing, the migration also nulls `mtime_ns` and `content_sha256` on active `ok` sources so [[src-tauri/src/storage.rs#classify_model_source_change]] cannot short-circuit them to fast/content-unchanged; the re-armed pass therefore genuinely re-parses pre-upgrade transcripts and stamps `derived_model_id` instead of leaving it null forever. [[src-tauri/src/model_usage.rs#apply_carry_forward_attribution]] stamps every parsed observation with its chain's running model — the last non-null, non-`<synthetic>` raw turn id; synthetic turns never update the running model, and rows before any model evidence stay null — while `raw_model_id` remains untouched replayable evidence. All analytics aggregation keys attribution on `derived_model_id`: `get_model_analytics` coverage, shares, and model rows; `get_model_history` series; `get_model_sessions` matching, primary, and distinct models; and `get_session_model_history` token totals. Segment and switch semantics intentionally stay on raw turn evidence.

Attributed coverage uses one token-bearing observation population: rows with derived attribution form the numerator, and rows still null after carry-forward form the unattributed remainder. A zero-token denominator stays unavailable. Tokenless turns still contribute model, session, first/last-seen, primary-model, and switch evidence.

[[src-tauri/src/storage.rs#Storage#get_model_history]] reads one matching snapshot into fixed, zero-filled UTC buckets: 5 minutes for 1 hour, 1 hour for 24 hours, 6 hours for 7 days, and 1 day for 30 days. It keeps attributed, unattributed, and optional provider-qualified selected-model series separate while excluding suppressed sources; like every other aggregate, series attribution keys on `derived_model_id`.

The aggregate, history, overview, and paged-session commands each open a short-lived read-only connection through [[src-tauri/src/storage.rs#Storage#open_model_analytics_reader]] and start their deferred transaction there, so none waits on the primary storage mutex or serializes behind ingestion writes; WAL still governs database-level writer/readers safely. That reader opens `SQLITE_OPEN_READ_ONLY` — the main-database write guard — and sets `temp_store=MEMORY`, a `mmap_size`, and a larger `cache_size` so the overview's in-memory scratch table stays off disk; it no longer sets `PRAGMA query_only`, which is incompatible with that temp table and redundant with the read-only open flag. The frontend's Models page now issues only the overview command; aggregate and history remain served for compatibility and tests.

[[src-tauri/src/storage.rs#Storage#get_model_usage_overview]] serves the Models-page overview from one read-only deferred-transaction snapshot as a [[src-tauri/src/models.rs#ModelUsageOverviewResponse]]. It scans the observation fact table once into an in-memory `scoped_overview` temp table — provider, session, derived model, kind, sidechain flag, effective cwd, timestamp, ordering tiebreakers, and precomputed token amount, bucket index, and day string — then drives every section off that materialized set instead of re-scanning the base table per section; the provider filter is applied as a sargable equality when set rather than an `OR … IS NULL` disjunction, and running-now resolves all providers in one window query instead of a per-provider loop. Only `represented_providers` and the global session count read the base tables separately, because their scope is deliberately provider-unfiltered. The response carries: range totals (sessions, projects, turns, tokens, coverage); per-model reach rows (sessions, projects, turns, primary-in count, days active, tokens and share); a running-now entry per provider pairing the latest contiguous model run with its predecessor; fixed-bucket per-model distinct-session activity (5-minute, 1-hour, 1-day, and 1-day buckets across the four ranges); a top-8 project × model session matrix; per-session model-count combinations with top co-occurrence pairs; and a parent-versus-subagent attributed token split keyed by source `is_sidechain`.

[[src-tauri/src/storage.rs#Storage#get_model_sessions]] pages sessions for one exact provider-qualified raw model without a catalog. Its checksummed opaque cursor fixes the half-open range, records the persisted model-data revision, and seeks by last activity descending, then binary provider/session identity ascending. Observation replacement, source pruning/failure visibility, deletion suppression, and model cwd changes advance that revision in their own commit; a later page rejects a stale cursor instead of mixing totals with unreachable rows. Each page derives selected-model usage, latest project/host context, range-scoped primary model, distinct models, independent chains, and turn-only within-chain switches from unsuppressed evidence.

[[src-tauri/src/storage.rs#Storage#get_session_model_history]] reads one provider/session over the same half-open range and unsuppressed source ownership. It returns coverage totals from every token-bearing observation, but only ordered turns create segments: repeated models compress, null-model turns compress into gaps and reset adjacency, and token-only rows neither create segments nor reset switches. Parent and subagent chain metadata must remain consistent, chains order parent first then first activity and binary chain ID, segment endpoints are the inclusive first/last turn timestamps, and primary-model ties use attributed tokens, turn count, then binary provider/raw ID. A session with no retained in-range observations returns a distinct storage-level not-found outcome for stale-row IPC handling.

Existing session, project, and host deletion transactions select model evidence through retained source ownership, delete observation children first, and leave each matching source suppressed at its last committed content hash. Session deletion matches the provider-qualified analytics/root session so parent and subagent sources are removed together. Project deletion selects a source when its retained `cwd` or any child observation `cwd` matches, then deletes every child of that source to preserve atomic replacement. Unchanged retries remain suppressed; only an atomic changed-content replacement restores evidence. Project rename updates exact-matching source and observation `cwd` fields independently in one transaction, preserving per-record cwd differences. Tauri wrappers perform snapshot reads and mutations off the async command worker, then emit `model-analytics-updated` only after commit. Model evidence has no independent TTL and is not coupled to token-hourly cleanup.

#### Model Analytics Test Specs

Behavior specs for the model-analytics parse and query paths, each covered by exactly one `// @lat:` reference at its representative test.

These unit specs exercise the current working-tree parsers and storage queries directly, without a live Tauri app. They complement the lifecycle prose above by pinning the exact edge-case counters, reconstructed token numbers, and suppression exclusions that the reconciliation and read paths must preserve.

##### Claude Transcript Adapter Edge Cases

[[src-tauri/src/model_usage.rs#parse_claude_model_usage_jsonl]] never lets one bad record abort a source; each edge case degrades to a counter or a null field instead.

Covers a truncated final line that preserves prior observations, sidechain turns missing an agent id, negative epoch timestamps, missing or non-string `type`, invalid token dimensions that still emit an unavailable-token observation, and model-id whitespace trimming where blank or missing ids stay null with no `unknown` synthesis.

##### Codex Cumulative Delta Reconstruction

[[src-tauri/src/model_usage.rs#parse_codex_model_usage_jsonl]] rebuilds per-turn deltas from cumulative `token_count` totals without underflowing or inventing identity.

Asserts exact input and cache-read decomposition across three monotonic turns, a decreasing counter that resets its baseline and emits the reset diagnostic, a trailing `session_meta` that still attributes earlier records under the two-pass invariant, and `turn_context` model evidence kept separate from token-only deltas.

##### Model Sessions Cursor Codec

[[src-tauri/src/storage.rs#encode_model_sessions_cursor]] and its decoder round-trip every cursor field and reject tampered cursors.

A flipped checksum nibble and a truncated envelope both decode to an [[src-tauri/src/storage.rs#ModelSessionsQueryError]] invalid-cursor error rather than a mismatched or partial page.

##### Model History Bucketing and Suppression

[[src-tauri/src/storage.rs#Storage#get_model_history]] and [[src-tauri/src/storage.rs#Storage#get_model_analytics]] bucket seeded evidence and exclude suppressed sources.

Observations seeded through the real [[src-tauri/src/storage.rs#Storage#replace_model_source|replace write path]] land attributed and unattributed tokens in their timestamp-containing fixed buckets with matching aggregate totals and coverage, while a source flipped to suppressed contributes to neither query, guarding the shared [[src-tauri/src/storage.rs#ACTIVE_MODEL_SOURCE_PREDICATE]].

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

- **tool_actions** — Tool invocation details for MCP (provider, message_id, session_id, tool_name, category, file_path, summary, full_input/output, plus `is_sidechain`, `agent_id`, and `parent_uuid` from migration 20). Indexed on provider/session, message_id, file_path, category, and the new provider+session+sidechain / provider+session+agent pairs. Retained transcript rows are committed only through source-owned snapshot replacement.
- **response_times** — Assistant response latency per provider/session turn (provider, session_id, timestamp, response_secs, idle_secs, plus the same migration-20 `is_sidechain`/`agent_id`/`parent_uuid` triple). Unique on (provider, session_id, timestamp).

#### Skill Usages

Recognized `SKILL.md` loads derived during the same Session Indexing extraction pass, keyed for analytics drilldowns by skill, provider, project, and host.

- **skill_usages** — One row per recognized skill load (provider, session_id, message_id, skill_name, skill_path, timestamp, tool_name, cwd, hostname). Unique on (provider, session_id, message_id, skill_name, skill_path, timestamp). Indexed on provider+timestamp, provider+session, skill+timestamp, and the migration-22 skill+cwd pair that powers per-project drilldowns. Migration 23 re-arms `skill_usage_reingest_pending` so historical sessions are replayed against the updated extractor without any schema change.

[[src-tauri/src/sessions.rs#extract_skill_accesses_from_tool_action]] recognizes three ingest shapes: Codex `exec_command` calls that read a `SKILL.md` path with `cat`/`head`/`tail`/etc., Claude `Read` calls against a `SKILL.md` path, and Claude `Skill` tool calls. The `Skill` arm normalizes the `skill` input via [[src-tauri/src/sessions.rs#skill_access_from_skill_tool_input]] by stripping any `plugin:` prefix so Claude rows merge with Codex's bare folder names (e.g. Claude `superpowers:using-superpowers` collapses onto Codex `using-superpowers`), and synthesizes a `skill://<raw>` path that preserves the original identifier for forensic drilldowns without colliding with filesystem paths.

`cwd` and `hostname` are populated in source-owned snapshots: Claude pulls `cwd` from each record's top-level field, Codex threads session-level `cwd` through every tool message in [[src-tauri/src/sessions.rs#ExtractedMessage]], and reconciliation captures the local hostname once per source. The HTTP message-ingest path leaves skill usage empty because its flattened payload has no tool-action detail.

#### Hook Invocations

Observed lifecycle-hook fires keyed for the Now-tab Hooks breakdown. Claude rows come from transcript extraction during the dual-emission pass; Codex rows arrive live via a dedicated HTTP endpoint because Codex rollouts do not log hook executions.

- **hook_invocations** — One row per observed hook fire (provider, session_id, chain_id, parent_chain_id, agent_id, is_sidechain, timestamp, hook_event, hook_matcher, tool_name, hook_identity, script_command_raw, exit_code, duration_ms, cwd, hostname, message_id). Owned hooks are unique on `(provider, source_key, chain_id, timestamp, hook_identity)`; source-less hooks use `(provider, session_id, chain_id, timestamp, hook_identity)`. `agent_id` remains attribution, not identity. Migration 30 rebuilds retained Claude rows through source reconciliation while preserving deduplicated source-less Codex observations.

[[src-tauri/src/sessions.rs#extract_hook_invocation_from_attachment]] recognizes records whose `attachment.type` begins `hook_` (covering `hook_success`, `hook_failure`, `hook_timeout`, `hook_blocked`) and maps `hookEvent`, `hookName`, `command`, `exitCode`, and `durationMs` onto the row. The Claude attachment's matcher half of `hookName` (e.g., the `Bash` in `PreToolUse:Bash`) becomes `hook_matcher`, and when the event is `PreToolUse` or `PostToolUse` it also fills `tool_name` so per-tool breakdowns work without a separate column lookup.

[[src-tauri/src/sessions.rs#canonicalize_hook_identity]] forms the aggregation key: paths inside `~/.config/quill/scripts/` or `~/.config/quill/codex/scripts/` collapse to `quill:<basename>` so Quill-managed rows have stable per-machine identities, `${CLAUDE_PLUGIN_ROOT}/<dir>/<file>` is kept verbatim because the unexpanded env-var prefix is the only stable plugin-scoped identifier the transcript provides, any other absolute path is reduced to its basename, and records with no `command` (older Claude transcripts) fall back to `hookName`. The verbatim command is preserved in `script_command_raw` (truncated to 2048 chars at a UTF-8 boundary) for forensic drilldowns.

Codex rows are inserted by [[src-tauri/src/storage.rs#Storage#store_codex_hook_observation]] from the `POST /api/v1/hooks/observed` background blocking task. Codex identity is event-scoped (`hook_event` with an optional `:tool_name` suffix when the event is `PreToolUse` or `PostToolUse`) because the deployed `hook-observe.cjs` observer (`src-tauri/codex-integration/scripts/hook-observe.cjs`) fires on every event without per-script attribution — Codex registers multiple scripts per event and the observer cannot identify which sibling script ran. Quill ships its own Codex hooks (`session-sync.cjs`, `context-capture.cjs`, `context-router.cjs`, `observe.cjs`, `report-tokens.sh`, plus the new `hook-observe.cjs` itself when `activity_tracking` is on); third-party Codex hooks fire but are not attributed beyond the event level. Codex telemetry is gated on the same `activity_tracking` IntegrationFeatures flag that already gates `observe.cjs`.

The endpoint accepts observations only for a ten-event whitelist (`PreToolUse`, `PostToolUse`, `SessionStart`, `UserPromptSubmit`, `SubagentStart`, `SubagentStop`, `Stop`, `PreCompact`, `PostCompact`, `PermissionRequest`) after the `SubagentStart`/`SubagentStop` lifecycle events were added, and length-caps `agent_id` exactly as it caps `tool_name`. [[src-tauri/src/models.rs#CodexHookObservation]] carries a serde-defaulted `agent_id: Option<String>` that `hook-observe.cjs` sends on every payload, and `store_codex_hook_observation` preserves that optional attribution while the source-less chain remains the incoming session id. The `hook_identity` computation stays event-scoped and unchanged.

[[src-tauri/src/storage.rs#Storage#delete_session_data]] cascades direct deletion through all five analytics tables. Project and host deletion use retained registry ownership and recorded live origin instead of row-local guesses.

Known limitation (Claude side): transcript extraction only sees hook fires that Claude Code records as `hook_*` attachment records, which it writes only for hooks that produce output or fail. Silent, always-on Quill hooks — `observe.cjs`, `session-sync.cjs`, `report-tokens.sh`, and `qbuild-guard.sh` — succeed without emitting output, so they never appear as attachments and are undercounted in the Hooks breakdown. Closing the gap needs a Claude-side live observer with a dedup design against the transcript-derived rows so the same fire is not double-counted; that observer is deliberately not shipped yet.

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

#### Source-Owned Transcript Analytics

Migration 30 establishes source and chain identity for transcript-derived analytics while preserving live rows as source-less data.

`session_events`, `response_times`, `tool_actions`, `skill_usages`, and `hook_invocations` carry nullable `source_key`, required resolved-root `session_id`, required native `chain_id`, and nullable `parent_chain_id`. Events and tool actions also carry stable `event_key` and `action_key` identities; missing tool IDs fall back to message/block identity or source-record/block ordinals. Partial unique indexes separate source-owned replay identity from source-less live identity, and each table has a `(provider, source_key)` lookup index.

Owned/live identities are respectively: `session_events` `(provider, source_key, event_key)` / `(provider, session_id, event_key)`; `response_times` `(provider, source_key, chain_id, timestamp)` / `(provider, session_id, chain_id, timestamp)`; `tool_actions` `(provider, source_key, action_key)` / `(provider, session_id, action_key)`; `skill_usages` substitutes `source_key` or `session_id` before `(message_id, skill_name, skill_path, timestamp)`; and `hook_invocations` substitutes the same owner before `(chain_id, timestamp, hook_identity)`.

`transcript_analytics_sources` stores canonical root/path ownership, fingerprints, last-good native and resolved identity, origin, inventory generation, processing diagnostics, and durable suppression. `live_analytics_sessions` stores project, cwd, and host origin for source-less analytics. Migration 30 sets `transcript_analytics_reingest_pending` for the later retained-source rebuild.

Migration 30 renames the five prior analytics tables to `*_legacy_v30` and rebuilds them around source identity. The archives are **retained**, not dropped: rows with no hostname, and local rows whose transcript Claude has since pruned, are neither provably remote nor guaranteed rebuildable, and retention beats copying a multi-GB database. Nothing queries an archive and no index is kept on one — legacy named indexes are dropped so a retained archive cannot collide with the rebuilt tables' index names. An archive holding no rows at all is dropped immediately, so fresh installs stay clean.

Carry-forward is limited to rows this machine can never rebuild from a local transcript. `hook_invocations` carries forward `provider = 'codex'` (Codex rollouts are never transcript-derived) or any row stamped with a hostname other than the local one, folding v29's `agent_id`-qualified duplicates onto the lowest rowid. `skill_usages` carries forward on the remote-host predicate alone. Only those two tables have a `hostname` column (migrations 22 and 27), so the other three cannot discriminate and retain everything in the archive. Every carried-forward session with a known hostname gets one `live_analytics_sessions` origin row, because project and host deletion reach source-less rows only through recorded live origin.

Migration 31 adds `project_path_renames`, the authoritative old-to-new path mapping used by retained replay and live ingestion, plus `(provider, chain_id, timestamp)` runtime ordering support. Rename chains collapse to one destination, so future transcript metadata cannot revert an explicit project rename.

[[src-tauri/src/lib.rs#run]] schedules whole-root transcript reconciliation during app setup, independently of Session Search or Analytics-window mounting. Blocking inventory and parsing run in the background under the same provider/root permits as live source work. An existing empty root proves an empty inventory; a missing, unreadable, or unavailable configured root is incomplete and cannot authorize pruning or marker clearance.

Reconciliation compares canonical source key/path and last-good status before reading content. Matching `mtime_ns` plus size advances only `seen_generation`; a changed fast fingerprint hashes one stable read and likewise preserves all five tables when the stored hash matches. Only new, changed, failed, or root-restamped sources parse and replace. Failures persist bounded retry diagnostics without changing the last-good fingerprint, identity, or child rows. While `transcript_analytics_reingest_pending` is set both short-circuits are bypassed, so an interrupted rebuild genuinely replays every retained source instead of trusting a stale fingerprint; the marker clears only after every root supplies complete inventory and prune proof.

[[src-tauri/src/storage.rs#Storage#refresh_unchanged_transcript_analytics_sources]] advances every unchanged source of one root in a single transaction rather than one per source — a real corpus collapses roughly 5,500 transactions into one. It returns the source keys whose rows did not update because the root generation moved under a concurrent run, so callers keep per-source stale-generation handling instead of one aggregate verdict; the single-source method is a thin wrapper over it.

`replace_transcript_analytics_snapshot` replaces all five owned analytics tables and the source registry in one transaction; valid empty snapshots remove only that source, while suppression and any insert failure leave prior rows intact. Owned inserts use `INSERT OR IGNORE` through statements prepared once outside their loops, matching the source-less live paths — an owned identity is the table's own dedupe key, so a legitimate repeat must not roll back the whole five-table snapshot. Distinct `cwd` values are resolved through the rename map once into a lookup table instead of once per skill and hook row. Registry upserts advance `seen_generation`; stale prepared generations are rejected before owned rows change. Parse or identity conflicts retain last-good registry state.

[[src-tauri/src/storage.rs#Storage#store_live_session_analytics]] atomically writes source-less runtime or hook rows with durable project, full-cwd, and host origin. Origin upserts preserve known fields with `COALESCE`; live event rows require unique message UUID identity and always use the incoming session as both root and chain.

Session, project, and host deletion removes all five analytics tables in one transaction. A retained project/host match expands through provider-qualified analytics roots before leaving every sibling source as a suppressed tombstone. Source-less project/host rows use only exact recorded live origin; direct session deletion also catches unmapped legacy live rows. Committed deletions emit `transcript-analytics-updated` only when transcript rows changed.

#### Transcript Analytics Test Specs

Behavior specs for the source-owned transcript pipeline — migrations, snapshot replacement, freshness classification, and identity resolution — each covered by exactly one `// @lat:` reference at its representative test.

These unit specs run against a real migrated SQLite file and real on-disk JSONL fixtures, without a live Tauri app. They pin the invariants the prose above states as guarantees: that a failed write leaves last-known-good rows intact, that a superseded generation cannot overwrite newer data, that carry-forward is limited to unrebuildable rows, and that identity resolution degrades to a counter instead of discarding a source.

##### Owned Snapshot Replacement Atomicity

[[src-tauri/src/storage.rs#Storage#replace_transcript_analytics_snapshot]] is one transaction across all five owned tables plus the registry, so a failure at the last statement must undo every delete and insert before it.

A replacement that violates the registry `CHECK` after the owned tables were already rewritten must restore the prior rows exactly, leave a sibling source of the same root untouched, and leave the registry generation unadvanced. The positive half asserts the other edge: an empty snapshot is a legal replacement that clears exactly one source and no sibling.

##### Snapshot Generation Guards

A snapshot prepared against generation `G` must never overwrite rows once the root has moved past `G`, however the advance was recorded.

Both paths are covered: the root generation setting advancing under a concurrent run, and a registry row already stamped newer than the replay. Each returns the `StaleGeneration` verdict rather than an error, leaves prior owned rows byte-identical, and does not restamp the registry.

##### Migration 30 Carry-Forward Scope

Migration 30 must carry forward exactly the rows this machine can never rebuild from a local transcript, and nothing else.

Codex hook rows and any row stamped with a foreign hostname survive as source-less data; local Claude rows, and rows with no hostname at all, exist only in the retained `*_legacy_v30` archives. The test also pins the v29 `agent_id` duplicate fold and the `live_analytics_sessions` origin row registered per carried-forward session, without which project and host deletion could never reach those rows.

##### Migration 30 Idempotence

Reopening a database that already ran migration 30 must skip it rather than renaming the rebuilt tables a second time.

The version-gated loop records exactly one `schema_version` row for the migration and reaches the same final version on a re-open, which is the only thing standing between a normal restart and a second destructive rebuild.

##### Migration 31 Idempotence

Reopening a database that already ran migration 31 must not re-enter it.

Migration 31 creates the rename table and the native-chain runtime index; re-entry must be a no-op that records no second `schema_version` row.

##### Migration 32 Idempotence

Reopening a database that already ran migration 32 must not re-enter it.

Migration 32 only creates the covering runtime index, but the same gate protects it, and a second recorded version row would misreport the schema ceiling.

##### Empty Legacy Archive Cleanup

A fresh install renames five empty tables and has no history worth keeping, so migration 30 must drop those archives instead of leaving dead tables in every new database.

Retention is a recovery mechanism for existing corpora only; without this the schema of every new install would permanently carry five unqueried tables.

##### Schema Ceiling Refusal

A database written by a newer build cannot be downgraded in place, so `Storage::init` refuses it instead of starting an app that would silently record nothing.

A version at [[src-tauri/src/storage.rs#MAX_SUPPORTED_SCHEMA_VERSION]] still opens; anything above it fails with the machine-readable `SCHEMA_TOO_NEW:` prefix callers match on. The refusal is a hard stop — the refused database's `schema_version` must be unchanged afterwards.

##### Project Rename Chain Collapse

Explicit project renames are collapsed on write so later transcript replay resolves an original path in a single lookup.

A chain `A → B → C` must resolve `A` straight to `C`, and the `A → B → A` round trip must terminate without leaving a self-referential row — the case that previously hard-errored on `CHECK(old_path != new_path)` because the collapse `UPDATE` rewrote its own predecessor into a self-row mid-statement. No surviving destination may itself be renamed.

##### Batched Unchanged Source Refresh

[[src-tauri/src/storage.rs#Storage#refresh_unchanged_transcript_analytics_sources]] advances many sources in one transaction without losing the per-source stale detection the per-source transactions used to provide.

It must return exactly the keys that did not update — including a row already stamped past the generation the batch was prepared against — while advancing the rest, and a mid-batch failure must roll the whole batch back rather than leaving a partially advanced root.

##### Runtime Totals Across Native Chains

`get_llm_runtime_stats` sums per native chain, so a parent and its sub-agent chains each contribute their own active interval.

Sibling sub-agents whose windows overlap in wall-clock time must not be merged into one interval, and no chain may be counted twice. The same fixture pins the `INDEXED BY idx_se_timestamp_chain` plan: the pinned query must still return these totals.

##### Workflow-Nested Sub-Agent Discovery

[[src-tauri/src/sessions.rs#SessionIndex#discover_claude_session_files_in]] recurses the whole `subagents/` subtree, so Workflow-spawned agents nested at `subagents/workflows/wf_<id>/agent-*.jsonl` are discovered alongside flat `subagents/agent-*.jsonl`.

A real projects dir with a parent `<uuid>.jsonl`, a flat sub-agent, and a workflow-nested sub-agent (leaner first record: no `cwd`/`gitBranch`/`version`) returns all three, tags only the two agents `is_subagent`, and drops a non-jsonl decoy at any depth. This is the discovery stage the direct-DB runtime test cannot exercise.

##### Claude Identity Anomaly Skipping

[[src-tauri/src/transcript_analytics.rs#resolve_claude_native_identity]] skips a stray record and counts it instead of rejecting the whole source.

A record copied across a fork with its prior `sessionId` is counted into [[src-tauri/src/transcript_analytics.rs#TranscriptRecordDiagnostics]] with the ordinal of the offending line, never adopted as identity, while a later conforming record still backfills a `cwd` the first record omitted.

##### Claude Layout Hint Mismatch

A retained-layout disagreement is one anomalous fact about an otherwise usable source, so it is counted rather than discarding every row.

A parent transcript discovered under the sub-agent layout hint still resolves its parent identity and records one `layout_hint_conflicts`; agreeing parent and sub-agent layouts record none. This replaced the former hard `LayoutConflict` rejection, mirroring `model_usage.rs::accept_claude_native_source`.

##### Claude Source Without Identity

Skipping anomalies must not degrade into accepting a source that has no valid identity at all.

Records with no `sessionId`, and a sidechain record with no `agentId`, are individually skippable — but a source made only of those still fails with `MissingNativeIdentity` rather than being stamped under a guessed root.

##### Freshness Fingerprint Short-Circuits

[[src-tauri/src/transcript_analytics.rs#classify_transcript_source_freshness]] decides reparse without extracting rows, and each short-circuit must fire on exactly its own condition.

Eight cases pin the ladder: identical mtime and size skip the digest entirely (including an in-place rewrite that preserves both, which stays trusted by design); mtime drift falls through to a digest that may match or reparse; and a missing stored digest, a `failed` status, a row with no last-good identity, or a row recorded for another path all refuse the fast path. Every unchanged verdict carries the current run's generation on its owed refresh.

##### Fast Path Avoids Source Reads

The fingerprint short-circuit must return without opening the file, not merely without parsing it.

A sparse fixture larger than [[src-tauri/src/transcript_identity.rs#RETAINED_TRANSCRIPT_MAX_BYTES]] would raise `SourceTooLarge` on any read, so an unchanged verdict is proof the contents were never touched — the property that makes startup reconciliation cheap on a corpus of thousands of unchanged sources.

##### Forced Reparse Bypasses Short-Circuits

`force_full_reparse` threads the durable reingest marker through classification and must bypass both short-circuits, without ever bypassing suppression.

The fingerprint fast path and a matching content digest both yield `Changed` under force while the same fixture short-circuits without it — otherwise the flag would prove nothing. Suppressed status and a suppressed digest marker are honoured under force, because suppression is a user deletion, not a staleness verdict.

##### Forced Reparse Reads The Source

Under force, classification must actually read a source whose fingerprint matches.

An oversized fixture with a matching stored fingerprint raises `SourceTooLarge`, which only an actual read can produce — distinguishing a real bypass from a flag that merely relabels the verdict.

##### Retained Transcript Size Cap

[[src-tauri/src/transcript_analytics.rs#read_stable_transcript]] enforces the 256 MiB retained cap from `metadata().len()` before allocating anything.

The guard is what keeps one pathological transcript from exhausting memory during a whole-root pass, and it must reject on apparent length rather than after a partial read.

##### Identity Comparison Excludes Cwd

[[src-tauri/src/transcript_analytics.rs#native_identity_matches]] compares only the fields that decide cross-source root membership.

A differing or absent `cwd` still matches, because `cwd` is descriptive origin and a last-good registry row can legitimately carry a different one than a fresh parse. Chain id, source session id, parent chain id, and agent id each independently break the match.

##### Commit-Time Identity Drift

Because the two reconciliation phases read at different times, a file that changes in between must not be stamped with the root resolved from its old identity.

Committing a source whose parsed identity no longer matches the inventoried one fails with `SourceIdentityDrift`, retaining last-known-good rows instead of silently reparenting them. A source that differs only by `cwd` still commits, so a moved checkout is not mistaken for drift.

##### Codex Identity Restatement And Cycles

[[src-tauri/src/transcript_identity.rs#resolve_codex_native_identity]] keeps the first child identity while tolerating consistent ancestor restatements and refusing everything else.

Thirteen cases cover root sessions, a collapsed ancestor chain, a restated child that fills a missing `cwd`, `forked_from_id` standing in for `parent_thread_id`, conflicting or dropped parents, unrelated second sessions, `A → B → A` and self-parent cycles that must terminate as conflicts rather than hang, and metadata too degenerate to yield any identity.

#### Code and Runtime Metrics

Tables for tracking active LLM session time, per-turn response latency, and cached git commit history per project.

- **session_events** — Runtime events carry `(provider, source_key, event_key, session_id, chain_id, parent_chain_id)` plus agent, timestamp, kind, UUID, and sidechain attribution. Migration 30 deduplicates owned rows by `(provider, source_key, event_key)` and source-less rows by `(provider, session_id, event_key)` through separate partial unique indexes.
- **response_times** (legacy for runtime card; still consumed by Sessions breakdown and sub-agent tree) — Per-turn latency carries the same source/root/chain lineage. Owned identity is `(provider, source_key, chain_id, timestamp)`; source-less identity substitutes `session_id` for `source_key`.

Migration 32 adds `idx_se_timestamp_chain(timestamp, provider, chain_id, is_sidechain, kind, session_id)`. Migration 31's `(provider, chain_id, timestamp)` index satisfies the runtime query's `ORDER BY` but buries `timestamp` third, so the range filter could never seek and every tick scanned the whole index with one rowid lookup per row. Leading with `timestamp` turns the filter into a range seek, and carrying the other five columns makes the index covering. The trade is a sort over the bounded window instead of a per-row heap fetch across the whole corpus. `get_llm_runtime_stats` pins the index with `INDEXED BY` because Quill never runs `ANALYZE`: with no `sqlite_stat1` the planner always prefers the sort-free migration-31 index. [[src-tauri/src/storage.rs#ensure_startup_indexes]] recreates `idx_se_timestamp_chain` on every open so the pin cannot fail on a database that lost it.

`get_llm_runtime_stats(range, scope)` sources from `session_events`. It walks events ordered by native `(provider, chain_id, timestamp)` and sums per-chain logical turns without unioning concurrent wall-clock intervals. A gap between an `asst_tool_use` and the next `user_tool_result` always counts as active time (clamped at 6 hours); any other gap above 300 seconds splits the current turn. Distinct sessions use provider-qualified resolved roots, while `parent_only` adds `WHERE is_sidechain = 0` so lineage metadata, not nullable agent fields, controls exclusion.

Codex extraction maps user and agent text, non-empty assistant `output_text`, reasoning, function/custom calls, and call outputs into the five runtime event kinds. Developer, user, administrative, and empty message items do not become assistant runtime events. Stable native identities or source record ordinals keep source replay deterministic.

Claude records may emit multiple ordered runtime events when one content array combines thinking, text, and tool blocks. Stable per-record ordinals distinguish each event. User tool results precede same-record text and assistant tool use follows thinking/text, preserving the tool-wait transition while retaining every semantic marker.

- **git_snapshots** — Cached git history per project (project unique, commit_hash, commit_count, raw_data).

#### Metadata

Key-value configuration and schema migration version tracking.

- **settings** — Key-value config storage.
- **schema_version** — Migration version tracking (currently v32). Migration 20 truncates `response_times` and `tool_actions` (regenerable from transcripts) and sets a `subagent_reingest_pending` flag in `settings`; migration 21 adds `skill_usages` and sets `skill_usage_reingest_pending` so the next [[backend#Session Indexing]] sweep clears `index_state.json` mtimes and re-reads JSONL transcripts to backfill recognized skill-use rows. Migration 22 adds `cwd` and `hostname` columns to `skill_usages` plus the `idx_skill_usages_skill_cwd` index, and re-arms `skill_usage_reingest_pending` so historical rows refill from JSONL transcripts on the next [[backend#Session Indexing]] sweep. Migration 26 adds the `session_events` table with its unique-on-identity index and sets a `runtime_event_reingest_pending` flag so the next [[backend#Session Indexing]] sweep also clears mtimes and refills `session_events` from JSONL transcripts. Migration 27 adds the [[backend#Database#Schema#Hook Invocations]] `hook_invocations` table with one UNIQUE expression index (identity + agent_id COALESCE) plus four secondary indices (provider+timestamp, provider+session, identity+timestamp, identity+cwd), and sets a `hook_invocation_reingest_pending` flag so the same sweep replays the new attachment extractor across every Claude transcript. Migration 28 adds normalized model observations, retained-source ownership, and the singleton state that separates backfill lifecycle, root completeness, source-total publication, and bounded progress counters. Migration 29 adds the nullable indexed `derived_model_id` attribution column to `model_usage_observations`, nulls the `mtime_ns`/`content_sha256` fingerprints on active `ok` sources so their transcripts are treated as changed, and re-arms `model_backfill_state` to pending under a bumped generation with a `migration` trigger so the next startup pass genuinely re-parses and re-attributes existing evidence. Migration 30 adds [[backend#Database#Schema#Source-Owned Transcript Analytics]] and its durable rebuild marker. Migration 31 adds authoritative project rename aliases and the native-chain runtime index. Migration 32 adds the covering runtime-window index described in [[backend#Database#Schema#Code and Runtime Metrics]]. Existing extractor flags remain until source reconciliation replaces their shared sweep lifecycle.

[[src-tauri/src/storage.rs#MAX_SUPPORTED_SCHEMA_VERSION]] is the highest migration this build knows how to apply, and `Storage::init` compares it against the recorded version before running any migration gate. A database written by a newer build fails initialization with a `SCHEMA_TOO_NEW:`-prefixed error rather than silently skipping every unknown migration and then failing every insert against columns it cannot satisfy. Nothing is written on the way past the guard.

## Tauri IPC Commands

The Tauri commands registered in [[src-tauri/src/lib.rs]] are grouped by feature.

### Usage and Token Commands (13)

Live usage and token analytics commands back provider quota, history, breakdown, and context-savings views.

`fetch_usage_data`, `get_usage_history`, `get_usage_stats`, `get_all_bucket_stats`, `get_snapshot_count`, `get_token_history`, `get_token_stats`, `get_token_hostnames`, `get_host_breakdown`, `get_session_breakdown`, `get_skill_breakdown`, `get_skill_project_breakdown`, `get_hook_breakdown`, `get_context_savings_analytics`.

The live-usage commands now treat utilization history as `(provider, bucket_key)` data instead of assuming a single global Claude bucket label.

Claude live usage comes from `https://api.anthropic.com/api/oauth/usage` via [[src-tauri/src/fetcher.rs#fetch_claude_usage]] using the local OAuth token. [[src-tauri/src/fetcher.rs#parse_buckets]] reads the flat top-level keys — `five_hour` ("5 hours") and `seven_day` ("7 days") still drive the aggregate windows and the tray indicator's short/weekly metrics. The API moved per-model weekly limits out of the flat `seven_day_sonnet`/`_opus`/`_cowork`/`_oauth_apps` keys (now returned as `null`) into a structured `limits` array; [[src-tauri/src/fetcher.rs#parse_scoped_weekly_limits]] reads each `kind: "weekly_scoped"` entry and emits a `UsageBucket` labeled by `scope.model.display_name` (e.g. `Fable` from the codenamed `omelette` slot), keyed `weekly_scoped_<model>` with `sort_order: 1`, deduped by label against the flat buckets. The `session` and `weekly_all` limits are skipped there because the flat keys already cover them. Because the API returns dropped keys as `null`, their last snapshot would otherwise linger as a ghost tile in the cached live view; [[src-tauri/src/storage.rs#Storage#get_latest_usage_buckets]] prunes any bucket whose newest snapshot is more than an hour older than the provider's most recent fetch (buckets from one fetch share a timestamp, so a paused provider prunes nothing), while history and stats queries read snapshots directly and are unaffected.

Codex live usage now comes from `codex app-server` `account/rateLimits/read` instead of transcript-only scraping. The backend normalizes the returned `rateLimitsByLimitId` map into provider buckets so Quill can store both the base Codex windows and model-specific limits such as Codex Spark in the same usage tables, while preserving the legacy base Codex bucket keys for history continuity. Model-specific `limitName` values are abbreviated for display via [[src-tauri/src/fetcher.rs#abbreviate_codex_model]] (e.g. `GPT-5.3-Codex-Spark` → `5.3-Spark`) by stripping the redundant `GPT-` prefix and `-Codex` infix. The stdio helper resolves the Codex executable path, then augments the user's login-shell `PATH` with the launcher and symlink-target directories so Node-backed npm installs still start from desktop-launched Quill. It ignores unrelated app-server frames such as the `initialize` response, and only deserializes the matching request id for the rate-limit call. If the direct app-server request fails, the fetcher falls back to transcript `token_count` `rate_limits`.

MiniMax live usage comes from the coding plan API at `api.minimax.io` via [[src-tauri/src/fetcher.rs#fetch_minimax_usage]]. It reads the API key from the SQLite settings table and parses the `model_remains` array into 5-hour and weekly `UsageBucket` entries, filtering out models with zero quota.

`get_session_breakdown` now accepts optional provider and limit arguments so Codex live views can request a provider-scoped active set without being crowded out by Claude sessions.

`get_session_breakdown` is provider-agnostic at the row level and rolls up parent + all sub-agent rows for each session: `total_tokens`, `turn_count`, `last_active`, and the input/output/cache columns sum across `is_sidechain ∈ {0, 1}`, and each row carries two new fields — `has_subagents: bool` and `subagent_count: u32` (COUNT DISTINCT `agent_id`) — that gate the [[features#Analytics Dashboard#Now Tab]] expandable tree. The `(provider, session_id, is_sidechain)` index added in migration 20 keeps each `UNION`'d branch on an index scan.

`get_skill_breakdown` returns recognized skill-use counts from the `skill_usages` table for the Analytics Now Skills tab. It accepts the active day range, optional Claude/Codex provider filter, all-time mode, and a capped limit; rows sort by `total_count DESC, skill_name ASC` and include provider sub-counts plus `last_used` and a `project_count` (`COUNT(DISTINCT cwd)`) surfaced in the Skills row metadata in the [[features#Analytics Dashboard#Now Tab]]. The frontend no longer gates expansion on `project_count > 1`, so single-project skills and skills with only null project metadata can open their project drilldown too.

`get_skill_project_breakdown` returns per-(project, hostname) counts for a single skill within the active analytics scope, used by the [[features#Analytics Dashboard#Now Tab]] Skills expand drilldown. It accepts `skill_name`, the active day range, optional Claude/Codex provider filter, all-time mode, and a capped limit; rows sort by `total_count DESC, last_used DESC, project ASC` after applying [[src-tauri/src/storage.rs#compute_subdir_parent_map]] subdir merge so `/a/b/c` folds into `/a/b` exactly like the Projects breakdown. Rows whose transcripts lack `cwd` are preserved as a null-project bucket so expanded counts sum to the parent skill total.

`get_context_savings_analytics` returns range-scoped summary totals, timeseries buckets, grouped breakdowns, and recent append-only events for the Analytics Context tab. Token values are approximate `ceil(bytes / 4)` estimates, while byte counts and event counts are exact where producers can measure them.

### Model Analytics Commands (6)

Model analytics IPC exposes validated aggregate, overview, history, paged-session, session-detail, and backfill operations through one structured, user-safe error contract.

[[src-tauri/src/lib.rs#get_model_analytics]] validates the fixed time range and optional existing Quill provider identifier before reading one analytics snapshot off the async command thread. [[src-tauri/src/lib.rs#get_model_history]] also validates an optional exact raw model identity and rejects provider-filter mismatches. [[src-tauri/src/lib.rs#get_model_usage_overview]] validates the same fixed range before reading the one-snapshot Models-page overview described in [[backend#Database#Schema#Model Analytics Evidence]] off the async command thread.

[[src-tauri/src/lib.rs#get_model_sessions]] validates the fixed range and exact provider-qualified opaque model ID, preserves the storage-owned 20-row null default, and clamps signed numeric limits to 1–100 before platform-independent conversion. Malformed, foreign, or stale opaque cursors return `invalid_cursor` without exposing cursor diagnostics.

[[src-tauri/src/lib.rs#get_session_model_history]] validates provider and range before loading one provider-owned session. Missing in-range retained evidence returns `not_found`, distinct from storage failure.

[[src-tauri/src/lib.rs#retry_model_history_backfill]] reserves scheduling before advancing the durable retry generation, returns current state when that retained pass is already scheduled or running, and treats an unowned persisted `running` row as interrupted work. A live-source owner can release the shared process permit before the pending pass starts.

Storage and blocking-task failures stay in local logs. All five commands return only the bounded serialized model analytics error envelope, and model IDs use shared opaque Unicode validation without a catalog or version allowlist.

### Indicator Commands (3)

`get_indicator_primary_provider`, `set_indicator_primary_provider`, and `get_indicator_state` keep one backend-owned indicator model shared across the tray title, tray summary rows, and the integrations menu.

`set_indicator_primary_provider` persists the configured provider in the settings table, recomputes the resolved indicator state from the shared usage cache or fallback rows, and emits `indicator-updated` so the tray summary and integrations menu stay synchronized without a second polling path.

### Project and Session Management (7)

`get_project_tokens`, `get_session_stats`, `get_project_breakdown`, `delete_project_data`, `rename_project`, `delete_host_data`, `delete_session_data`.

`delete_session_data` deletes token snapshots and all five transcript analytics tables for the selected `(provider, session_id)` pair, plus every model source owned by that provider-qualified analytics/root session. Model observation children are removed before retained source fingerprints become suppressed, preventing a retry from resurrecting unchanged deleted evidence. Project rename updates retained/live ownership and cwd-bearing analytics in the same transaction, then preserves that choice through `project_path_renames` on future replay.

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

`get_llm_runtime_stats(range, scope)` accepts an optional `scope: "all" | "parent_only"` argument and sources from the `session_events` table (see [[backend#Database#Schema#Code and Runtime Metrics]] for the gap-classification rules and the 5-minute idle threshold / 6-hour tool-wait ceiling). `None` or `"all"` preserves the existing behavior across every chain; `"parent_only"` adds `WHERE is_sidechain = 0` so the headline runtime card on the [[features#Analytics Dashboard#Now Tab]] can report parent-thread cost without sub-agent traffic inflating it. The `idx_se_provider_session_sidechain` index covers the filter. The IPC return shape (`LlmRuntimeStats { total_runtime_secs, turn_count, session_count, avg_per_turn_secs, sparkline }`) is unchanged from migration 25's contract.

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
| `transcript-analytics-updated` | lib.rs | `()` | Transcript analytics committed, renamed, or deleted |
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

When a transcript is reprocessed, Quill coalesces repeated `notify` requests per session and applies each Tantivy rewrite under one writer lock with a single commit. Retained SQLite analytics are independently coalesced by canonical `(provider, source_key)` and replaced across all five tables in one transaction. The mtime sweep deletes existing session docs unconditionally before reinserting, even on first sight of a file, so hook-driven `notify` ingestion that ran before the file was tracked in `index_state.json` cannot stack duplicate copies on top.

Skill usage is derived by [[src-tauri/src/sessions.rs#extract_skill_accesses_from_tool_action]], which recognizes read-like loads of a `SKILL.md` file and derives the skill name from that file's parent directory. Retained rows are owned and replayed by canonical source, and the extractor does not infer skills from assistant prose, available-skill lists, or skill-file maintenance edits. Flattened `/sessions/messages` payloads contain no tool-action detail and emit no skill rows.

The Claude walker descends into `<projectSlug>/<session-uuid>/subagents/agent-*.jsonl` in addition to the flat parent transcript at `<projectSlug>/*.jsonl`, and each `DiscoveredSessionFile` carries an `is_subagent` flag so downstream extraction can tell the two apart. Claude extraction reads `isSidechain`, `agentId`, and `parentUuid` from each JSON record. Codex retained analytics preserves the first child `session_meta.id` as `chain_id` and resolves `parent_thread_id`, falling back to `forked_from_id`, as `parent_chain_id`; later ancestor restatements cannot replace child identity. Per-session Claude sub-agent files live in one flat directory — multi-level hierarchy is reconstructed at query time via `parent_uuid` chains rather than nested filesystem layout.

The HTTP API also accepts provider-tagged notify and direct message ingestion. Local Claude full-transcript sync is Stop-scoped, while direct message ingestion still appends atomically for incremental remote updates. BM25 scoring plus snippet generation power the shared search UI with provider filters and badges.

[[src-tauri/src/sessions.rs#validate_retained_notify_source]] validates one `notify` path against only its configured provider root, canonical containment, and supported layout without walking transcript history. Quill admits a canonical source to model and transcript reconciliation before session-keyed search coalescing; a resolvable path that fails the stricter retained-source policy still coalesces for search only, preserving the indexing contract. Direct message payloads append Tantivy documents and atomically store source-less runtime rows plus recorded live origin through [[src-tauri/src/storage.rs#Storage#store_live_session_analytics]].

### Search Scoring

Query parsing applies per-field BM25 boosts so concrete artifacts outrank noisy metadata.

The default-search field weights are: `files_modified` (4.0), `code_changes` (2.5), `commands_run` (2.5), `tool_details` (1.5), `content` (1.0), and `tools_used` (0.5). Without these boosts, equal weighting plus BM25 length-normalization let short fields like `tools_used` (where every session contains tokens like `Edit` and `Bash`) dominate ranking. The derived `display_text` field is kept in the parser at boost 0.1 only so Tantivy's `SnippetGenerator` — which filters terms by field — can highlight matches against it; it is a superset of `content + code_changes + commands_run + tool_details` and would otherwise double-count every term. Query-parser errors from `parse_query_lenient` are logged at debug level instead of being silently discarded.

## Claude Code Inference Client

[[src-tauri/src/cc_client.rs]] is the single inference surface for the app. Every LLM call (learning streams + synthesis, memory optimizer, prose compression) spawns the `claude` CLI in headless one-shot mode rather than making a direct HTTP request to Anthropic.

Public surface: [[src-tauri/src/cc_client.rs#invoke_typed]] for schema-validated structured output and [[src-tauri/src/cc_client.rs#invoke_text]] for free-form prose. Model routing: Sonnet 4.6 (pinned by full model name `claude-sonnet-4-6`) for all work — pattern extraction, learning synthesis (single-model since feature 005 US5 T060/H-7; no rolling `sonnet` alias, stable cost attribution), and prose work. The `Haiku`/`Sonnet` enum variants are retained dead-code-only for easy revert. `--json-schema` is unreliable (the CLI does not enforce it), so typed calls do not use it. `invoke_typed` instead embeds the JSON Schema in the prompt, grants the headless agent a `Write`-only tool sandboxed to a per-call temp dir, and has it write the result to `out.json`; Quill reads that file and `serde_json::from_str::<T>` is the sole validation (missing/invalid → `SchemaValidationFailed`, no app-side retry). `invoke_text` is unchanged (free-form, total tool isolation).

The `claude` binary is located via [[src-tauri/src/config.rs#resolve_command_path]] — the same cached, login-shell-aware resolver used for provider CLI detection — so it picks up Anthropic's `claude migrate-installer` target and auto-refreshes when the user triggers a PATH rescan. Each invocation runs `claude -p --output-format json --model <alias> --append-system-prompt <preamble> --tools "" --disable-slash-commands --no-session-persistence --setting-sources "" --exclude-dynamic-system-prompt-sections` and pipes the prompt body on stdin, joined with `wait_with_output` in a single future so a large prompt cannot deadlock against the child's stdout. The subprocess is isolated from the user's interactive Claude Code configuration (their hooks, slash commands, plugins, CLAUDE.md auto-discovery, and session history are all suppressed) and runs with `CLAUDE_CODE_*`, `ANTHROPIC_*`, and `NODE_OPTIONS` scrubbed from the inherited environment.

No app-side retry, no `Retry-After` interpretation, no rate-limit backoff. Each invocation has a 300-second hang-detector timeout (via `tokio::time::timeout` + `kill_on_drop`). Errors are categorized into eight stable variants — `ClaudeCodeMissing`, `ClaudeCodeTooOld`, `NotSignedIn`, `RateLimited`, `SchemaValidationFailed`, `TimedOut`, `Spawn`, `BadEnvelope` — each producing a user-facing message that names the cause and the actionable remediation. When `BadEnvelope` fires on a successful exit (status=0, stdout unparseable), the error string is enriched with the exit status and the first 1024 chars of stderr so silent-exit failures (e.g. a sandboxed launcher catching a denied path and `process.exit(0)`-ing without writing the envelope) stay diagnosable from the `learning_runs.error` / `optimization_runs.error` column. See `specs/003-cc-inference-migration/contracts/cc-client.md` for the full contract.

On top of the in-process flag isolation (defense in depth, kept verbatim), the spawned `claude` is wrapped with the best-available OS-level confinement because it processes untrusted captured content. [[src-tauri/src/cc_client.rs#apply_sandbox]] runs as the last step of `build_command`, rewrapping the fully-formed command. Linux is a three-tier chain — **Landlock** (primary; in-process kernel LSM, no user namespaces, no AppArmor entanglement) → **Bwrap** (subprocess fallback for kernels without Landlock or hosts where bwrap is still permitted) → **None** (unconfined, honestly recorded, actionable diagnostic emitted). macOS uses `sandbox-exec` with a deny-by-default SBPL profile (reads scoped to system/runtime prefixes + the resolved claude/node tree, **no** `$HOME`/`~/.claude`/`~/.config`/project access; writes confined to the per-call temp dir); Windows relies on the existing `kill_on_drop` Job Object association (documented best-effort). The Linux primary tier applies a Landlock ruleset built by [[src-tauri/src/cc_client.rs#build_ruleset]] from a [[src-tauri/src/cc_client.rs#LandlockPolicy]] (ABI v3 declared with `CompatLevel::BestEffort` so older Landlock-capable kernels degrade access rights cleanly) via a forked-child pre-spawn hook on the `tokio::process::Command`'s underlying `std::process::Command::pre_exec` — the hook runs `prctl(PR_SET_NO_NEW_PRIVS, 1, …)` then `RulesetCreated::restrict_self()` in the child between `fork` and `execve` so Quill itself stays unrestricted; the ruleset grants RO `path_beneath` rights to `{/usr, /bin, /sbin, /lib, /lib32, /lib64, /etc, /opt, /nix-if-present, /proc, /sys, /dev, /run/systemd/resolve, /run/dbus, claude_install_root, ~/.claude.json, ~/.claude}` and RW rights to `{per-call TempDir, /dev/null}`, with absent optional paths silently skipped (mirrors bwrap's `--ro-bind-try`). The host pseudo-filesystems `/proc`, `/sys`, `/dev` are in the RO set because Landlock has no mount namespace (unlike bwrap's `--proc`/`--dev`/`--tmpfs` which inject fresh ones) — denying them makes the launcher's Bun runtime SIGILL at startup on `readlink(/proc/self/exe)` / `open(/dev/urandom)` / `open(/proc/cpuinfo)`; the trade-off vs bwrap is that `/proc/N/*` exposes other PIDs' cmdline/environ to the subprocess. The `~/.claude.json` + `~/.claude/` RO entries deviate from spec 007's original "no `$HOME` / no `~/.claude`" design — required because claude 2.1.152's Bun launcher reads its config + cached OAuth credentials from those paths during startup and, on EACCES (vs. ENOENT), silently `process.exit(0)`s with empty stdout/stderr (no actionable error). Read-only `path_beneath` lets the launcher authenticate without giving the subprocess write access to session history, hooks, plugins, or the credentials file; the rest of `$HOME`, `~/.config`, and project trees stay denied. The `/run/systemd/resolve` + `/run/dbus` RO entries are required for the spawned child's DNS resolution when Quill spawns from a Tokio runtime (which is always the case in production — Quill is a Tauri/Tokio app): `/etc/resolv.conf` is a symlink to `/run/systemd/resolve/stub-resolv.conf` on systemd-resolved hosts, and the Tokio-context resolver follows it (a std-context resolver happens to succeed without `/run` access — see R-H). Both `/run` paths are tiny transient state, contain no user data, and `path_beneath_rules` silently skips them on hosts without systemd-resolved. See `specs/007-landlock-inference-sandbox/research.md` R-G + R-H for the bisection evidence. [[src-tauri/src/cc_client.rs#build_command]] also exports `TMPDIR=<per-call dir>` and `NODE_COMPILE_CACHE=<per-call dir>` on the typed-call path so the launcher's transient writes route into the already-allowed RW dir instead of `/tmp` (no-op under bwrap, which gives a private tmpfs `/tmp`, and under `None`). The Bwrap fallback's argument construction is byte-for-byte the same as before feature 007 (deny-by-default filesystem, no `$HOME`/`~/.claude`/`~/.config`/project access, a single RW bind of the per-call temp dir); only its *position* in the chain moved from primary to first fallback. The previous `unshare`-based `ProcessOnly` tier introduced by feature 006-A is **retired** — it required the same `CLONE_NEWUSER` capability AppArmor blocks on Ubuntu 24.04+, so it was theatrical on exactly the hosts that broke bwrap, with no FS-confinement value either way. When the chain falls all the way through to `None`, a process-wide one-shot diagnostic ([[src-tauri/src/cc_client.rs#emit_no_confinement_diagnostic]], guarded by `OnceLock<()>`) is emitted to both `log::error!` (visible in the `tauri dev` terminal) and the per-call log channel that lands in `learning_runs.logs` (visible in run-history detail) — two templates: a **generic FR-014** message when neither mechanism is available at detection, and an **AppArmor-specific FR-015** message when bwrap was attempted and failed because of Ubuntu 23.10+'s default `kernel.apparmor_restrict_unprivileged_userns=1` policy (detected by [[src-tauri/src/cc_client.rs#classify_bwrap_failure]] returning [[src-tauri/src/cc_client.rs#BwrapBrokenCause]]`::AppArmorRestrictUserns` after substring-matching bwrap stderr against `"setting up uid map: Permission denied"` or `"loopback: Failed RTM_NEWADDR: Operation not permitted"`); a process-wide `OnceLock<BwrapBrokenCause>` latch prevents re-spawning the same known-broken bwrap on subsequent calls in the same Quill process. Network is deliberately preserved on every branch (no net namespace / `network-outbound` allowed, no Landlock network rules) — the CLI makes the model API call itself. Helper binaries are still resolved via a `std::env::split_paths` PATH scan plus absolute fallbacks; the one new approved dependency is `landlock` 0.4.4 (Apache-2.0/MIT, kernel-feature author's crate, Linux-only target-cfg). Confinement **never fails closed**: if Landlock build/probe errors, the chain falls through to bwrap; if bwrap is absent or latched-broken, the flag-isolated command runs unchanged and inference continues; the reduced state is recorded. See `specs/005-learning-system-hardening/research.md` R-7.6, `specs/006-learning-hardening-followups/research.md` R-A, `specs/007-landlock-inference-sandbox/research.md` R-A..R-F, `specs/007-landlock-inference-sandbox/contracts/landlock-sandbox.md` C-A..C-E, and FR-005/SC-013.

The structured `--output-format json` envelope returned by every call carries per-call metadata (input/output tokens, cache stats, model id, durations, cost, stop reason, permission denials) that is captured into [[src-tauri/src/cc_client.rs#InferenceCallMetadata]] and persisted on the parent run record's `inference_metadata` JSON column for both `learning_runs` and `optimization_runs`. The record also carries a `sandbox` field — one of the closed write vocabulary `{"landlock", "bwrap", "sandbox-exec", "job-object", "none"}` ([[src-tauri/src/cc_client.rs#SandboxKind]]) — recording the applied OS confinement for every call on both the success and `failed_metadata` paths so SC-013 (confinement state recorded for 100% of analysis runs on every platform) is verifiable. The tag is honest about the boundary: [[src-tauri/src/cc_client.rs#sandbox_tag_is_fs_confined]] (single source of truth, keyed on the stable tag) is `true` for `landlock`/`bwrap`/`sandbox-exec` (real deny-by-default filesystem confinement) and `false` for `job-object`/`none`; the classifier stays **tolerant of any legacy tag** including the retired `"process-only"` and pre-feature-006 `"unshare"` (both → `false`) and any unknown future tag (→ `false`), so historical rows decode forever without migration (feature 007 contract C-D). Feature 006 Follow-up A's operator-disclosure plumbing is preserved unchanged: [[src-tauri/src/storage.rs#decode_inference_metadata]] projects a derived `confinement` (`{ sandbox, fs_confined }`) onto each `RunInferenceCall` and an `all_fs_confined` rollup onto `RunInferenceSummary`, and [[src/components/learning/RunHistory.tsx]] renders a distinct amber marker plus the remediation hint for any run that recorded a not-FS-confined call (FS-confined and legacy/no-inference runs render unchanged).

[[src-tauri/src/fetcher.rs]] is the only remaining consumer of the Claude Code OAuth credential in the codebase. It powers the [[features#Live Usage View]] by polling `api.anthropic.com/api/oauth/usage` and was intentionally not migrated as part of feature 003 (see `specs/003-cc-inference-migration/spec.md` FR-015). A 401 from that endpoint is treated as a stale access token (a muted "Paused" state), so the only logged-out warning path runs [[src-tauri/src/config.rs#claude_logged_in]], which spawns `claude auth status --json` UNCONFINED to read the `loggedIn` boolean without touching the credential store — see [[data-flow#Usage Bucket Fetching]] step 8d.

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

- `#[cfg(unix)]` — Process signal handling (SIGUSR1 for restart), nix crate for signal/process, `setsid` + env-var handshake for update-driven relaunch (see [[architecture#Architecture#Single Instance]])
- `#[cfg(target_os = "linux")]` — `/proc/<pid>/exe` parent-binary detection in [[src-tauri/src/lib.rs#detect_parent_same_binary_pid]] for the one-time relaunch transition
- `#[cfg(target_os = "macos")]` — Keychain integration for credential reading; `libc::proc_pidpath` for parent-binary detection in [[src-tauri/src/lib.rs#detect_parent_same_binary_pid]]
- Cross-platform path resolution via `dirs` crate

## Error Handling

All IPC commands return `Result<T, String>` for frontend-friendly errors. Internal functions use `.map_err()` chains with context. No panics in public APIs.

`log::error!()` / `log::warn!()` for debugging. Graceful degradation throughout.

Live-usage IPC carries one structured exception to the plain-string contract: `UsageData.provider_errors` is `Vec<UsageProviderError>`, where each entry pairs the provider with a typed [[src-tauri/src/models.rs#ProviderErrorKind]] discriminator (`Network`, `Config`, `Auth`, `RateLimit`, `Server`) alongside the human-readable message. `RateLimit` is currently inert (429 responses persist a silent cooldown timestamp rather than emitting a visible pill); the variant is reserved so future code can opt into a "Rate-limited" pill without another payload change. The frontend uses the discriminator to collapse all `Network` entries into a single offline pill in [[src/components/UsageDisplay.tsx]] (so a multi-provider outage produces one signal instead of N) while keeping per-provider rows for non-network failure modes. The flow that drives this — transport-error cooldown, exponential half-jitter backoff, and the on-success counter clear — is documented under [[data-flow#Data Flow#Usage Bucket Fetching]] step 8b.

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
