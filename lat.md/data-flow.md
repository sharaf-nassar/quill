# Data Flow

The system has seven primary data pipelines connecting hook scripts, the HTTP server, the database, and the frontend.

## Token Reporting Pipeline

Hook scripts capture token usage from Claude Code and Codex sessions and report it to the widget for real-time tracking.

1. Claude Code or Codex session produces transcript/state with token counts
2. Provider hook script (`report-tokens.sh`) extracts tokens and POSTs to `POST /api/v1/tokens` with Bearer auth
3. [[src-tauri/src/server.rs]] validates, rate-limits, and inserts into `token_snapshots` table
4. Server emits `tokens-updated` Tauri event
5. Frontend hooks (`useTokenData`, `useAnalyticsData`) receive event and refresh via IPC
6. Hourly cleanup task aggregates snapshots into `token_hourly` by provider/host for historical queries

Each provider script searches newest-first with a binary reverse reader using fixed 64 KiB chunks. Memory stays bounded by one chunk plus the current logical record; invalid UTF-8, invalid JSON, non-object records, and malformed provider payloads are skipped so an older valid usage sample can still report. Every consumed token leaf must be a non-boolean integer from 0 through 100,000,000. Codex prefers a valid `last_token_usage`, falls back to valid `total_token_usage` in the same record, then continues to older records.

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

1. **Claude path (transcript-derived).** Claude Code writes a `type:"attachment"` JSONL record for hook invocations, carrying `hookEvent`, `hookName`, `command`, `exitCode`, `durationMs`, and `parentUuid`. [[src-tauri/src/sessions.rs#extract_hook_invocation_from_attachment]] peels off hook records during the shared parse, canonicalizes commands via [[src-tauri/src/sessions.rs#canonicalize_hook_identity]], and source-owned snapshot replacement commits them with native sub-agent lineage. Tantivy indexing performs no analytics writes.
2. **Codex path (live observer).** Codex rollouts do not record hook executions, so the installer registers `src-tauri/codex-integration/scripts/hook-observe.cjs` on every one of the ten Codex hook events when `activity_tracking` is enabled. On each fire the script reads stdin, builds a `{provider, session_id, agent_id, hook_event, tool_name, cwd, ts, hook_matcher}` payload (session id resolved through a `session_id || conversation_id || id` fallback), and POSTs to `POST /api/v1/hooks/observed` with bearer auth and a 1.5-second local timeout.
3. The HTTP handler in [[src-tauri/src/server.rs]] validates the ten-event whitelist plus length caps and ISO-8601 timestamp shape, fast-acks `202 Accepted`, and dispatches the insert via [[src-tauri/src/storage.rs#Storage#store_codex_hook_observation]] on a `tokio::task::spawn_blocking`. Codex identity is event-scoped (`hook_event` plus optional `:tool_name`) because the observer fires per-event, not per-script.
4. The server emits `hooks-observed-updated` after a successful Codex insert. [[src/hooks/useBreakdownData.ts#useBreakdownData]] subscribes to that channel (alongside `tokens-updated` and `sessions-index-updated`) with a 1-second debounce so Codex live fires tick the Hooks breakdown within ~2 seconds without flooding the IPC layer.
5. Migration 30 clears legacy Claude transcript-derived hook rows, retains deduplicated source-less Codex observer rows, and arms source-owned transcript reconciliation. Startup rebuilds Claude hooks from retained inventory; Codex hook rows continue accruing prospectively through the observer endpoint.
6. The frontend [[src/components/analytics/BreakdownPanel.tsx]] renders one row per `hook_identity`, keeps `quill:` prefixes visible in the identity text, and includes a help affordance that explains the Claude/Codex tracking asymmetry.

The privacy gate is the existing `activity_tracking` feature flag: toggling it off removes `hook-observe.cjs` and its ten `[[hooks.<Event>]]` config entries from `~/.codex/config.toml`, stopping new Codex observations. Claude-side ingestion is unaffected because Quill never writes anything to Claude transcripts — the data flows from the user's transcript to Quill, not the other way around.

## Model Observation Reconciliation

Retained Claude and Codex transcripts become source-owned model observations without coupling model identity to Session Search indexing.

[[src-tauri/src/sessions.rs#enumerate_retained_jsonl_source_roots]] canonicalizes each configured provider root and contained supported JSONL path. [[src-tauri/src/sessions.rs#canonical_source_key]] combines the stable provider-root key with native path bytes, so full inventory and [[src-tauri/src/sessions.rs#validate_retained_notify_source|live notify validation]] address the same source without lossy path or cross-provider collisions.

[[src-tauri/src/model_usage.rs#parse_claude_model_usage_jsonl]] emits Claude assistant turns from explicit `message.model` plus any dimensions on that record. [[src-tauri/src/model_usage.rs#parse_codex_model_usage_jsonl]] keeps explicit `turn_context` model evidence separate from normalized cumulative `token_count` deltas. Missing or invalid identity stays null on the raw turn record, preserving replayable evidence without inventing an `unknown` model. After adapter parsing, [[src-tauri/src/model_usage.rs#apply_carry_forward_attribution]] stamps each observation's `derived_model_id` with its chain's running model — the last non-null, non-`<synthetic>` raw id, which synthetic turns never update — so within-chain token rows inherit attribution and only rows before any model evidence stay unattributed; aggregation keys on the derived id while segment and switch semantics stay on raw turn evidence. Forked Codex subagent rollouts declare their parent through `parent_thread_id` or `forked_from_id` and embed `session_meta` copies restating the whole ancestor chain; [[src-tauri/src/transcript_identity.rs#resolve_codex_native_identity]] walks that declared chain at any depth, keeps the child identity, and marks a source conflicted only for identity claims outside the chain, which alone exclude it from analytics.

[[src-tauri/src/model_usage.rs#prepare_model_source_reconciliation]] stages complete source reads, content hashes, provider parsing, and graph resolution into an owned plan before any replacement transaction. A transcript larger than a fixed byte cap is treated as an unreadable source instead of being read into memory, so one pathological file cannot exhaust the indexer. Filesystem layout hints can report conflicts but cannot override transcript-native parent metadata.

`prepare_model_source_reconciliation` enumerates and fingerprints the entire affected root, so it is reserved for the full backfill pass. The live queue instead calls [[src-tauri/src/model_usage.rs#prepare_scoped_model_source_reconciliation]], which loads each affected root's persisted source inventory with one indexed query — no filesystem walk — and only reads, hashes, and parses the queued changed sources. [[src-tauri/src/model_usage.rs#build_scoped_source_root_graph]] seeds the resolution graph from those staged sources plus every persisted sibling's stored chain metadata, and [[src-tauri/src/model_usage.rs#stabilize_scoped_root_graph]] re-parses only descendants whose resolved analytics root actually moves, so editing one transcript no longer restages the whole tree while cross-source chain and subagent resolution stay correct. The live plan captures no prune proofs.

When an ancestor changes a retained descendant's resolved analytics root, preparation reparses that otherwise unchanged descendant before writes begin. [[src-tauri/src/model_usage.rs#commit_next_model_source_batch]] commits bounded source batches from the stable plan so a worker can yield without losing graph context; errors return prior committed outcomes. Preparation captures prune proofs only for roots complete in that inventory, and pruning also requires every planned source commit. Event status is read before mutation, each source commit remains atomic, and post-commit `model-analytics-updated` delivery is best-effort and storage-free.

Deletion keeps each removed source fingerprint as durable suppression. Reconciliation skips unchanged suppressed content; only one successful atomic replacement of changed content clears suppression and restores observations. All aggregate, history, paging, and chain queries exclude suppressed ownership, so attribution coverage and empty-state scope follow the same lifecycle.

The [[src-tauri/src/storage.rs#Storage#get_model_analytics|aggregate]], [[src-tauri/src/storage.rs#Storage#get_model_history|history]], overview, and [[src-tauri/src/storage.rs#Storage#get_model_sessions|paged-session]] reads use separate read-only deferred transactions rather than the primary connection mutex. They can read WAL snapshots concurrently with each other and committed reconciliation batches without changing per-response snapshot semantics, so the Sessions drill-down no longer stalls behind ingestion writes.

[[src-tauri/src/lib.rs#enqueue_model_usage_live_source]] admits validated retained transcripts to a managed queue keyed by provider and canonical source key. Repeated notifications for one source coalesce, while sibling sources remain independent. [[src-tauri/src/lib.rs#drain_model_usage_live_queue]] acquires the atomic process permit, applies capped failure backoff, and moves blocking discovery and reconciliation off Tauri's async command threads. It retains the permit across bounded commit batches and yields between them, preserving one prepared graph decision while keeping the runtime responsive.

After storage initializes, [[src-tauri/src/lib.rs#run]] resets interrupted running history to pending and reserves one nonblocking migration/resume worker. Explicit [[src-tauri/src/lib.rs#retry_model_history_backfill]] uses the same reservation before changing durable state, so concurrent retries are idempotent and an unowned persisted `running` row is safely recovered; live work can finish under the shared permit before the pending retained pass starts.

[[src-tauri/src/sessions.rs#SessionIndex#startup_scan]] independently enumerates and admits every retained source before Session Search reads its mtime cache. Search extraction, unchanged mtimes, and later scan failures therefore cannot suppress model fingerprint reconciliation; partial root diagnostics remain bounded while discovered siblings keep their provider and owning root.

[[src/hooks/useModelAnalytics.ts#useModelAnalytics]] advances one frontend refresh generation after committed model events or fallback polls. Aggregate and history requests independently collapse same-identity signals received in flight into one post-commit deferred refresh, preventing live backfill events from continuously superseding accepted scope data; changed identities still supersede immediately. [[src/components/analytics/ModelsTab.tsx#ModelsTab]] passes the shared generation to selected-model paging and lazy session history, so loaded pages replay while expanded rows refetch independently. A stale history `not_found` is shown before composition hides only its exact provider/session row in the active range/model scope; old-scope callbacks cannot remove current rows, and successful page reconciliation releases hide markers for absent rows.

## Session Indexing Pipeline

Session transcripts are indexed for full-text search with enriched metadata, while provider-aware side tables keep tool and latency data distinct.

1. Claude Code writes session JSONL files to `~/.claude/projects/`, and Codex writes rollout transcripts to `~/.codex/sessions/`
2. When Session Search opens, [[src-tauri/src/sessions.rs]] scans both provider transcript roots incrementally by mtime
3. Provider hook scripts can also post `POST /api/v1/sessions/notify` with JSONL path plus provider metadata, while incremental remote sync can push `POST /api/v1/sessions/messages`
4. `notify` requests acknowledge first, then feed independent search and analytics schedulers; remote `messages` requests acknowledge only after source-less analytics commits, while Tantivy indexing remains asynchronous and best effort
5. Local Claude full-transcript sync now runs on Stop instead of every PostToolUse event, so full-file reindexing happens at a stable boundary instead of after each tool completion
6. Provider-specific parsers enrich messages: Claude tool blocks and Codex function/custom tool calls become tools_used, files_modified, code_changes, commands_run, and tool details
7. Indexed into Tantivy with fields: provider, message_id, session_id, content, role, project, host, timestamp, git_branch, plus enriched metadata
8. Retained runtime, response, tool, skill, and hook rows persist in provider-aware SQLite snapshots owned by canonical transcript source; remote message and hook pushes use separate source-less identities
9. Frontend search queries use TF-IDF weighted scoring with snippet generation
10. Faceted search pre-aggregates provider, project, and host counts

### Enrichment

Each message is enriched during indexing by parsing tool call inputs and outputs.

Claude `Edit`, `Write`, `MultiEdit`, and `NotebookEdit` tool calls become `code_changes`, Bash becomes `commands_run`, and Read/Grep/Glob become `tool_details`. Codex `apply_patch` (as either a custom-tool call or a function call), `exec_command`, and `write_stdin` map to `code_changes`/`commands_run` respectively, and MCP or auxiliary tool calls become searchable `tool_details` — except MCP tools whose input carries a clear file-write shape (`old_string`/`new_string`, or `file_path`/`path` plus `content`), which are classified `code_changes`.

For every `code_change` action, per-action `lines_added`/`lines_removed` are computed here from the FULL, untruncated tool input before `full_input` is capped at 10KB, then stored in the `tool_actions` columns of the same names (migration 33). This avoids the prior undercount where large edits truncated past 10KB failed to re-parse and counted zero. MultiEdit sums line counts across its `edits`, NotebookEdit counts `new_source` lines (removed for `delete` mode, added otherwise), and apply_patch counts `+`/`-` patch lines.

### Dual Emission for Runtime Tracking

The same parse pass that produces `ExtractedMessage` for the search index also produces `ExtractedEvent` for the [[backend#Database#Schema#Code and Runtime Metrics]] `session_events` table.

The search index keeps its existing filter (it drops `tool_result`-only user messages and empty assistant blocks), while the event stream carries every non-meta `user`/`assistant` line with a non-empty timestamp — classified by content shape into `user_text`, `user_tool_result`, `asst_text`, `asst_thinking`, or `asst_tool_use`. Retained-source reconciliation owns transcript analytics persistence; Tantivy startup and `/sessions/notify` indexing never delete or insert those rows. The `/sessions/messages` remote-push handler stores source-less events through [[src-tauri/src/storage.rs#Storage#store_live_session_analytics]], consuming ordered event kinds when supplied and retaining the one-event `(role, content, tools_used)` heuristic for older clients.

Claude remote sync keeps one wire message per timestamped provider record while sending every runtime role in canonical order: user tool result then text, or assistant thinking then text then tool use. This retains mixed-block semantics without duplicating response-time messages. Text and tool names remain flattened for search, while a narrow `assistant_tool_use` type hint supports older servers. Native `sessionId`, `isSidechain`, `agentId`, and `parentUuid` fields cross the wire explicitly; incomplete native child identity is never rewritten as parent activity. Native and fallback IDs use disjoint `claude:native:` and `claude:fallback:` namespaces; UUID-less rows derive stable identity from a root-session-plus-source hash and absolute source-line ordinal.

Pending rows post in at most 500-message requests, matching the server contract. Each 2xx response checkpoints the next unsent absolute source line, or EOF, so skipped lines around an accepted chunk do not replay and a partial failure retries only remaining messages. Rejection, timeout, and transport failure never advance the unaccepted range.

`splitSourceLines` reports whether the transcript ended on a newline, and a trailing unterminated line is never acknowledgeable. Without that, a cursor could jump past a record the provider was still writing, and the completed record would be lost silently and permanently. `postJSON` likewise separates permanent failures (4xx other than 408 and 429) from retryable ones. A row-attributable `400` is bisected down to the single poison record, which is logged and dropped so the rest of the session syncs; envelope-level `400`s and `401`/`403`/`404`/`413` hold the cursor instead, because no amount of bisecting can isolate an envelope problem. Previously any `400` wedged a session's sync forever with no signal. Identity guards log unconditionally and send the homogeneous prefix rather than discarding the whole batch, and client-side pre-filters mirror the server's own validation (RFC3339 timestamps, `MAX_CONTENT_LEN`, field length caps) so avoidable rejections never reach the wire. Cursor writes use an owner-specific temporary file, fsync and close it, then rename it over the prior cursor so interruption cannot expose an empty or prefix value. The next exclusive lease owner removes crash-orphaned cursor temps for that source before resuming.

A stable per-transcript-source lease root contains random owner-specific candidate directories, so a parent and its sub-agent files never share a cursor or lock. Acquisition scans before and after candidate creation; concurrent candidates either leave one winner or all back off. Owners heartbeat and remove only their own inode-and-token-qualified path, so an expired owner's cleanup cannot address a replacement path. Stale candidates are likewise pruned only by their unique path.

Feature 009 adds a third sibling to this dual emission: the same Claude JSONL walk peels off `type:"attachment"` records whose `attachment.type` begins `hook_` (e.g. `hook_success`, `hook_failure`, `hook_timeout`, `hook_blocked`) via [[src-tauri/src/sessions.rs#extract_hook_invocation_from_attachment]], producing one [[backend#Database#Schema#Hook Invocations]] row per fire. Sub-agent transcripts inherit the `is_sidechain=1` and `agent_id` columns automatically because the attachment extractor reads the same record-level fields the message extractor does. Codex rollouts do not emit attachment records, so Codex hook telemetry instead arrives live via `POST /api/v1/hooks/observed` from a deployed observer script.

### Source-Owned Analytics Snapshots

Retained-source reconciliation runs in two phases so peak memory is one source rather than the entire retained corpus.

[[src-tauri/src/transcript_analytics.rs#reconcile_transcript_source_root]] phase one resolves cross-source native identity only, retaining a few hundred bytes per source and dropping every record and byte it read, so the whole-root graph is known before the first commit. Phase two parses, stamps, commits, and drops one snapshot at a time. The cost is a second read of each source that actually needs committing; the prior design held every snapshot for both roots resident simultaneously.

[[src-tauri/src/transcript_identity.rs#resolve_codex_native_identity]] preserves the first Codex child identity while accepting consistent ancestor restatements and rejecting conflicting parents or cycles. [[src-tauri/src/transcript_analytics.rs#resolve_claude_native_identity]] skips anomalous Claude records — a sidechain record restated in a parent file, or a record copied across a fork — counting them into `TranscriptRecordDiagnostics` instead of rejecting the source; only a source with no valid identity at all fails, and a provider that owns no retained transcripts fails with `UnsupportedProvider`. [[src-tauri/src/transcript_analytics.rs#parse_transcript_analytics_source]] performs a bounded stable read, decodes JSONL once with original line ordinals, and emits source-owned runtime, response, tool, skill, and hook rows. Response and idle pairs are computed only within that source and chain. [[src-tauri/src/transcript_analytics.rs#stamp_analytics_root]] validates native attribution before applying the cross-source resolved root.

Because the two phases read at different times, [[src-tauri/src/transcript_analytics.rs#native_identity_matches]] re-checks the parsed identity against the inventoried one before stamping. A file that changed in between would otherwise be stamped with a stale root and silently reparented, so drift is a source failure that retains last-known-good rows. `cwd` is excluded from that comparison as descriptive origin, so a moved checkout is not drift.

Startup reconciliation allocates durable per-root generations, resolves each provider-qualified native chain independently, and resumes the migration-armed historical rebuild until all available roots finish. While the reingest marker is set, `force_full_reparse` bypasses both the mtime/size fast path and the content-digest short-circuit; suppression is honoured regardless and never bypassed. Unchanged sources owe only a `seen_generation` bump, flushed for the whole root in one transaction. Unrelated sessions sharing one provider directory retain distinct roots, and Tantivy indexing performs no analytics writes.

Failure is isolated per source. `RootReconciliationFault` separates a source-scoped failure from a `RootUnavailable` root that simply produces no prune proof and from a `Database` fault — failure of the bounded diagnostic upsert itself — which is the only one that abandons the run, because after it nothing can retain last-known-good state. Reconciliation replaces only source-owned rows, and pruning is gated on enumeration completeness alone: a failed source cannot block it, because [[src-tauri/src/storage.rs#Storage#record_transcript_analytics_source_failure]] refreshes `seen_generation`, so the `seen_generation < ?` prune query can never select it. [[src-tauri/src/storage.rs#Storage#prune_transcript_analytics_sources_for_root]] surfaces a row-decode failure while collecting prune keys instead of swallowing it, so a partial key set cannot look like a complete one.

Live notifications use a separate provider-plus-source queue. Scoped reconciliation combines the changed source with persisted sibling identities and reparses only descendants whose resolved root moves. A provider/root permit serializes full inventory-through-prune and scoped prepare-through-commit lifecycles, while registry writes reject older generations.

Per-source capped backoff lets healthy siblings continue after one source fails. A successful changed snapshot emits `transcript-analytics-updated`, refreshing runtime and breakdown views without relying on Session Search events.

### Live Analytics Origin

Source-less analytics retain only origin fields explicitly supplied by their live producer, so project and host deletion never guesses session membership.

[[src-tauri/src/storage.rs#Storage#store_live_session_analytics]] commits `/sessions/messages` runtime rows with project, full cwd, hostname, and native chain identity, or `/hooks/observed` hook rows with cwd, beside one `live_analytics_sessions` mapping. The optional cwd and chain wire fields preserve older clients. Later writes merge non-null origin fields.

The HTTP handler validates every flattened message before persistence: UUIDs are trimmed, non-empty, and unique; roles are `user` or `assistant`; timestamps parse as RFC3339; explicit child rows require consistent root, chain, parent, and agent IDs; supplied event kinds must be a canonical role-specific subsequence. Any malformed row rejects the whole batch with `400 Bad Request`. Message UUID plus explicit per-message event ordinal provides stable live event identity, while response timing still consumes one original message row. Storage repeats identity and contiguous-ordinal checks inside one transaction. A `2xx` response means that transaction committed, so the bridge may advance its durable cursor; storage failure returns `500`, while missing or failed Tantivy indexing cannot discard committed analytics.

Explicit deletion resolves retained rows through `transcript_analytics_sources` and source-less rows through `live_analytics_sessions`, always preserving provider identity. Project and host matches first resolve provider/root pairs, then expand to every sibling source under those roots. Direct session deletion also removes legacy source-less rows lacking a mapping. Retained registry rows become durable suppression tombstones, so unchanged or changed files cannot recreate deleted analytics without an explicit restore workflow.

Project rename updates both ownership registries plus cwd-bearing skill and hook rows in one transaction. Migration 31 records a collapsed authoritative path alias, which retained replacement and live writes resolve before persistence; later transcript metadata therefore cannot restore the old project. [[src-tauri/src/storage.rs#Storage#rename_project]] retires a reversed alias with an explicit `DELETE` before collapsing predecessors: a reversal (`A→B` then `B→A`) otherwise made the collapse `UPDATE` rewrite its own predecessor into a self-row mid-statement, which `CHECK(old_path != new_path)` rejected before the trailing self-row sweep could run. Rename and deletion emit `transcript-analytics-updated` only when affected analytics changed.

Raw candidate paths retry only while ownership validation is unavailable; invalid candidates are dropped. Validated canonical sources enter an independent admission retry that retains model and transcript work separately until both queues accept it, even when Session Search is unavailable. New generations wake stale sleeps, and terminal or capped retries are logged.

### Sub-Agent Transcripts

The Claude file walker recurses the whole `<projectSlug>/<session-uuid>/subagents/` subtree, in addition to the flat parent transcript, so sub-agent activity flows through the same enrichment and storage path.

It collects every `.jsonl` at any depth: flat `subagents/agent-*.jsonl` plus Workflow-spawned agents nested one level deeper at `subagents/workflows/wf_<id>/agent-*.jsonl` (~20% of agents on a heavy Workflow user). The walk is bounded to that subtree so unrelated nested JSONLs never sneak in.

Each sub-agent file becomes a separate ingest entry. Claude uses `agentId` as native child chain and `sessionId` as parent; Codex forked rollouts use their first child `session_meta.id` and `parent_thread_id` or `forked_from_id`. Resolved-root `session_id` is stamped later without replacing either provider's child identity. Workflow-layout agent records are leaner than flat ones — their first record omits `cwd`, `entrypoint`, `gitBranch`, `promptId`, and `version` — so identity resolution reads every non-linkage field as optional and never assumes those keys exist.

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
13. On undo: restore from backup_data and set status=undone; provider instruction updates first reject stale live content
14. `memory-files-updated` event triggers UI refresh

Single and group execution acquire the shared integration mutation guard whenever a suggestion targets a provider instruction file. The guard covers staleness validation, filesystem changes, and status updates. Undo of an instruction update additionally requires the live file to equal the stored proposed content before restoring its backup, protecting newer installer or user edits.

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
8a. Claude 429 responses persist a rate-limit cooldown timestamp in the settings store, and subsequent refreshes honor that cooldown before retrying the live API. While the cooldown holds the poll serves the last persisted snapshot and pushes `ProviderErrorKind::Stale` via [[src-tauri/src/lib.rs#push_stale_error]] — both on the fresh 429 that arms the cooldown and on every subsequent poll it short-circuits — so [[src/components/UsageDisplay.tsx]] renders a single muted "Showing cached data" pill (slate, not red) rather than presenting the cached rows as live (the offline pill wins if both are present). A 401 from the usage API is treated as a stale access token, not a logout: [[src-tauri/src/fetcher.rs#fetch_claude_usage]] returns a `Paused` kind with a neutral message, the poll pushes `ProviderErrorKind::Paused` via [[src-tauri/src/lib.rs#push_paused_error]], and [[src/components/UsageDisplay.tsx]] shows a muted "Paused" badge with any cached rows still rendered (and the badge alone on a first-run empty view) — never a login prompt or red error. To keep both guarantees, [[src-tauri/src/lib.rs#build_usage_data]] excludes `Paused` and `Stale` when picking the top-level `error`, so a stale-token or first-run-429 poll with no cached rows yet never surfaces a red "Failed to load usage data" — the muted badge/pill shows over an empty state instead. The red "Run: claude /login" guidance is reserved for a confirmed logout: no local credentials AND `claude auth status` reporting `loggedIn: false` (see 8d).
8b. Transport failures (DNS, connect refused, pre-response timeout) on Claude or MiniMax persist a per-provider network cooldown computed by [[src-tauri/src/lib.rs#compute_network_backoff]] — half-jitter exponential with a 60-second base, 30-minute cap, doubled per consecutive failure. The cooldown lives in the backend; the frontend `setInterval` poller keeps firing every 3 minutes but each call is short-circuited inside `refresh_usage_cache` and returns cached rows without a live HTTP request. The backend `tokio` loop hits the same short-circuit. No live request is made for either polling path during the cooldown. The poll pushes a typed `ProviderErrorKind::Network` so [[src/components/UsageDisplay.tsx]] can render a single consolidated "Offline — showing cached data" pill instead of one red banner per provider. On any successful fetch, both cooldown timestamps and the consecutive-failure counter clear. The fast offline signal itself comes from [[src-tauri/src/config.rs#http_client]]'s 5-second connect timeout (15-second overall), so reqwest never hangs on a dead network.
8c. The kind classification originates in the fetcher: [[src-tauri/src/fetcher.rs#ClaudeUsageError]] exposes a Claude `kind` (`Credentials`/`Paused`/`RateLimited`/`Request`/`Api`/`Parse`) and [[src-tauri/src/fetcher.rs#MiniMaxUsageError]] exposes its own (`Unauthorized`/`RateLimited`/`Request`/`Api`/`Parse`). The polling layer in [[src-tauri/src/lib.rs]] maps Claude `Request` to `ProviderErrorKind::Network` (driving the network cooldown), `RateLimited` to a rate-limit cooldown plus a muted `ProviderErrorKind::Stale` (pushed on the fresh 429 and on every subsequent poll the cooldown short-circuits via the `UseCachedAsStale` decision from [[src-tauri/src/lib.rs#check_provider_cooldown]]), `Paused` (401, stale token) to the muted `ProviderErrorKind::Paused`, `Credentials` (no local token) to `Config` only after the logout confirmation in 8d (otherwise `Paused`), and the remaining variants to `Server`. MiniMax still maps `Unauthorized` to `Auth`. The mapping itself lives in the pure helpers [[src-tauri/src/lib.rs#classify_claude_error_kind]] and [[src-tauri/src/lib.rs#classify_minimax_error_kind]] so the match can be unit-tested without touching storage. Cooldown bookkeeping (skip-on-active, write-on-error, clear-on-success) goes through the per-provider [[src-tauri/src/lib.rs#ProviderCooldownKeys]] struct: each provider declares a constant value of that struct (`CLAUDE_COOLDOWN_KEYS`, `MINIMAX_COOLDOWN_KEYS`) wiring its four setting keys to the shared helpers [[src-tauri/src/lib.rs#check_provider_cooldown]], [[src-tauri/src/lib.rs#clear_provider_cooldowns]], [[src-tauri/src/lib.rs#write_rate_limit_cooldown]], and [[src-tauri/src/lib.rs#record_network_failure]]. Adding a new provider is a typed `<Provider>UsageError` in `fetcher.rs`, a fifth setting-key quartet, a constant `<PROVIDER>_COOLDOWN_KEYS` value, and a `classify_<provider>_error_kind` mapping — no further branching needed.
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
