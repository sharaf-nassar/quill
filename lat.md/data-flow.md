# Data Flow

The system has seven primary data pipelines connecting hook scripts, the HTTP server, the database, and the frontend.

## Token Reporting Pipeline

Hook scripts capture token usage from Claude Code and Codex sessions and report it to the widget for real-time tracking.

1. Claude Code or Codex session produces transcript/state with token counts
2. Provider hook script (`report-tokens.cjs`) extracts tokens and POSTs to `POST /api/v1/tokens` with Bearer auth
3. [[src-tauri/src/server.rs]] validates, rate-limits, and inserts into `token_snapshots` table
4. Server emits `tokens-updated` Tauri event
5. Frontend hooks (`useWidgetSeries`, `useCodeInsights`) receive the event and refresh via IPC through the shared five-second-or-longer `useCachedInvoke` fan-out; insight comparisons read exactly two selected periods
6. Hourly cleanup task aggregates snapshots into `token_hourly` by provider/host for historical queries

Each provider script searches newest-first with a binary reverse reader using fixed 64 KiB chunks. Memory stays bounded by one chunk plus the current logical record; invalid UTF-8, invalid JSON, non-object records, and malformed provider payloads are skipped so an older valid usage sample can still report. Every consumed token leaf must be a non-boolean integer from 0 through 100,000,000. Codex prefers a valid `last_token_usage`, falls back to valid `total_token_usage` in the same record, then continues to older records.

### Data Shape

`TokenReportPayload` carries provider, session id, hostname, timestamp, token counts, and cwd.

That keeps combined analytics provider-safe while still sharing one token pipeline.

Analytics session drill-down uses the same provider plus session id pair when requesting token history, compact token stats, or session deletion, so identical ids from different providers stay isolated.

Hook-reported tokens still flow into `token_snapshots` keyed by the parent `session_id` — Claude sub-agents share the parent's session id on disk, so each row also carries `is_sidechain`/`agent_id`/`parent_uuid` from migration 20. The [[backend#Tauri IPC Commands#Usage and Token Commands (13)]] `get_session_breakdown` rollup aggregates parent and sub-agent rows at query time so a sub-agent's tokens count toward the parent session's totals, and `get_llm_runtime_stats(scope = "parent_only")` is available when the widget runtime readout needs to exclude the sub-agent traffic instead.

An accepted Pi usage event also writes one `token_snapshots` row for any known lifecycle origin, ephemeral or ordinary. Pi has no hook script and no transcript-derived token importer, so this push is the only producer of Pi rows in that table — gating it on ephemeral origins hid every persisted Pi session from the token series and the readouts derived from it. Event-UUID dedupe on the observation insert gates the write, so replay cannot double tokens. Sessions unions zero-token ephemeral origins until usage arrives and returns their additive badge flag; ordinary origins still need token evidence.

Sessions ranking clamps token snapshots newer than the latest matching root terminal hook to that hook. Fresh transcript identities join retained candidates before the final limit, preserving stored metrics when newer transcript activity reopens them. Stop-cycle token bookkeeping cannot reopen a row; strictly newer transcript or response activity can.

## Database Maintenance Pipeline

Two manual paths quiesce ingest before touching SQLite: compaction reclaims
space with VACUUM, and retention pruning deletes rows past an age window and
then compacts inside the same lease.

1. The Performance settings control invokes `compact_database` and subscribes
   to `compact-database-progress` while its request is in flight.
2. The backend takes [[src-tauri/src/lib.rs#begin_ingest_quiesce]], which
   blocks app-owned mutations behind the reader/writer gate and makes new HTTP
   ingest requests return retriable `503` responses.
3. The command emits a disk-space progress phase, performs free-space
   preflight, and reports a structured `skipped` result if there is not enough
   room for a safe rebuild.
4. On success, [[src-tauri/src/storage.rs#Storage#vacuum_database]] opens its
   dedicated SQLite connection, emits the compaction phase, and runs VACUUM.
5. A successful VACUUM runs [[backend#Database#Database compaction#Bounded Query Planner Analysis]]
   under the same lease, checkpoints its statistics write, and refreshes the
   long-lived writer's prepared planner state. A skipped VACUUM never analyzes.
6. Releasing the guard permits pending internal writes to continue; the
   command then emits `compact-database-finished` with either the completed
   before/after footprint or the safe skip reason.
7. The settings surface renders progress and the terminal result inline;
   external hook clients retry their rejected ingest instead of losing it.

### Retention Pruning Path

The same quiesce gate carries the destructive path: preview, explicit consent,
chunked deletes bounded by an age cutoff, and a compaction that turns the freed
pages into freed bytes without letting an ingest write land between the halves.

1. The Performance control writes the window with `set_retention_policy`, then
   invokes `preview_retention`. Both retention commands acquire through
   [[src-tauri/src/lib.rs#try_begin_ingest_quiesce]], so a lease already held by
   compaction is a structured skip rather than an unbounded wait.
2. Preview derives the cutoff, scans all three target tables under a `Counting rows`
   heartbeat on [[src-tauri/src/lib.rs#RETENTION_MAINTENANCE_PROGRESS_EVENT]],
   and returns exact per-table row counts, the counts it will keep because their
   timestamps are not byte-comparable, the affected surfaces, and the cutoff the
   run must echo back.
3. The user chooses `Archive & prune` or `Prune without archive`. The control
   re-previews to mint a fresh cutoff and calls `run_retention_maintenance`;
   stale confirmations are refused before the lease is taken.
4. [[src-tauri/src/retention_engine.rs#run_retention_delete_phase]] opens its own
   maintenance connection and materializes the doomed rowids. When requested,
   [[src-tauri/src/retention_engine.rs#write_retention_archive]] atomically
   publishes a JSONL sidecar containing every preview-counted row before any
   delete, including the non-conforming rows the database keeps.
5. The engine preflights free disk after the optional archive, then proves every
   affected model group and finalized runtime source equals its hourly refold.
   Missing coverage refuses before `retention.watermark` moves. Each chunk marks
   covered rollups `raw_pruned=1`, clamps runtime state past deleted rowids,
   writes transcript daily counters, and deletes raw in one transaction. The
   partial runtime turns stay raw, while a fully doomed turn is sealed into the
   hourly authority first. Runtime stats never merge daily counters.
6. The maintenance connection is closed, then
   [[src-tauri/src/storage.rs#Storage#vacuum_database]] runs under the same
   lease and emits `Compacting database`. Its preflight is independent of the
   delete preflight, so a run that removed rows but cannot afford a rebuild is a
   completed prune with a skipped compaction, reported rather than hidden.
7. The audit record is rewritten to `retention.last_run` on the completed,
   partial *and* skipped paths, then
   [[src-tauri/src/lib.rs#invalidate_analytics_after_retention]] drains the five
   analytics caches and emits `transcript-analytics-updated` — a DELETE never
   advances a cache high-water mark, so nothing else would retire a pre-prune
   payload. The lease is released and `retention-maintenance-finished` carries
   the structured result.
8. Consumers pick it up from there: the settings panel renders the terminal
   state and the durable audit record,
   [[src/hooks/useRetentionCutoff.ts#useRetentionCutoff]] re-reads the policy on
   the finished event so degradation banners state the new cutoff, and the
   watermark makes the deletion durable by filtering the next snapshot
   replacement's inserts rather than by trusting nothing to reparse.

## Learning Analysis Pipeline

Tool-use observations, git history, and recent session history are analyzed by LLMs to discover reusable behavioral patterns.

1. Provider hook script (`observe.cjs`) captures PreToolUse/PostToolUse events. The Claude script applies a low-signal pre-tool skip list (`Read`, `Glob`, `Grep`, `Bash`, `LS`, `WebSearch`, `WebFetch`, `Agent` — post-phase still records outcomes for those) and a high-signal post-tool allowlist (`Bash`, `Edit`, `Write`, `MultiEdit`, `NotebookEdit`); other post-tool calls — Read/Grep/Glob, `mcp__quill__*`, `mcp__lat__*`, `ToolSearch`, `Skill`, `AskUserQuestion`, etc. — return early because their outcomes carry no behavioral signal for the rule learner. The Codex script captures canonical `Bash` (including unified `exec_command`) and `apply_patch` tool events. A 30-day audit showed the unfiltered post-tool firehose contributed ~50% of `observations` rows with zero downstream value
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

Hook fires are durable SQLite audit rows; the newest root terminal hook is also advisory negative evidence, while positive session and agent state still comes from [[data-flow#Data Flow#Live Session Tracker|the live tracker]].

1. Claude transcript attachments provide general audit history. Under `activity_tracking`, `observe.cjs` posts learning observations plus root Stop, StopFailure, and SessionEnd terminal evidence from the existing sync groups.
2. Codex rollouts log no hook executions, so its generic observer is registered on eight observed events (`PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `UserPromptSubmit`, `SessionEnd`, `Stop`) under the same gate, carrying hostname, session fallbacks, optional agent id, and event-scoped audit identity. Pi maps its lifecycle events onto this same vocabulary.
3. `POST /api/v1/hooks/observed` accepts Claude, Codex, or Pi, validates the supported event, length caps, and ISO-8601 timestamp, and rejects anything else with `400`. It holds no live state, so a rejection costs nothing beyond the dropped audit row.
4. The handler fast-acks `202 Accepted` and writes [[backend#Backend#Database#Schema#Hook Invocations]] on a blocking worker through [[src-tauri/src/storage.rs#Storage#store_hook_observation]], emitting `hooks-observed-updated` on success and `hooks-ingestion-error` on failure.
5. [[src/hooks/useBreakdownData.ts#useBreakdownData]] declares `hooks-observed-updated` for Sessions and Hooks. Mounted keys join the shared five-second fan-out. Sessions IPC merges retained and observed-only rows, then projects each row's newest valid root Stop, Claude StopFailure, or SessionEnd. A terminal at or after `last_active` turns the green indicator off; strictly newer activity reopens it.

Root Stop and SessionEnd are observed for both providers, plus Claude StopFailure. SessionStart and subagent lifecycle stay transcript-derived; crashes and power loss retain the five-minute fallback.

Disabling activity tracking removes both managed observer paths and clears the whole fold; provider disable clears only that provider's half of it. Both mutations emit `hooks-observed-updated` after the clear so mounted Sessions caches refresh immediately, and the next sweep rather than the next read rebuilds whatever is still live. A session silent past [[src-tauri/src/live_tracker.rs#IDLE_AFTER]] is evicted rather than kept as distrusted state, so an uncovered row reads as null instead of a false zero and memory stays bounded by sessions still producing evidence.

Codex config changes flow through parsed TOML: install snapshots the active custom or default Codex home, records prior feature and MCP environment values, replaces only Quill commands, then reconciles positional trust keys against pre/post `hooks/list` metadata. Verification reparses the file, rejects retired Quill shell-hook commands omitted from `hooks/list`, and checks exact enabled/trusted handlers; startup repair then migrates stale registrations, which is also what prunes lifecycle registrations written by an older Quill. Uninstall targets the recorded home and restores the captured user values.

Every lifecycle `hooks/list` call uses a process-only local-provider override, so enable, repair, feature reapplication, verification, and uninstall never initialize the configured model provider or rewrite its TOML tables.

## Live Session Tracker

[[src-tauri/src/live_tracker.rs#LiveTracker]] owns Claude/Codex/Pi transcript folds plus protocol-2 Pi lifecycle state, so a Sessions read costs a map lock instead of a directory walk. It is the sole owner of liveness and open-agent membership.

`POST /api/v1/pi/track` accepts only bearer-authenticated protocol-2 lifecycle envelopes. Lifecycle commits before live mutation and answers typed dispositions; model, tokens, activity, and agent rails arrive from the persisted Pi session file. Validation finishes before mutation. Hostnames reject control characters and normalize once to the lowercase short storage key. Only session start may create state; later lineage updates require its current process, shutdown removes it, and stale shutdown cannot close a newer startup/reload continuation. Replacement starts remove their prior id first. Demo mode mutates nothing.

Protocol-2 lifecycle metadata carries normalized host, process instance, reporter version, Quill build, and capability digest, but only the protocol version gates decoding. Migration 47 drops the obsolete `pi_reporter_health` table and saturation settings; Integrations derives no reporter-generation status or remediation from transport heartbeats.

Migration 46 sets `pi_spool_cleanup_pending`, and [[src-tauri/src/server.rs#start_server]] does not spawn the legacy drain while that durable marker exists. No spool lifecycle, lineage, runtime, or usage record is imported into the new owner; persisted Pi sessions are the reconciliation source. Spool retirement waits only for persisted-source reconciliation, not reporter reload or exact-generation acknowledgement.

[[src-tauri/src/live_tracker.rs#LiveTracker#apply_paths]] folds the Claude, Codex, and Pi transcripts the filesystem watcher reports. Each file carries the byte offset already consumed, so steady state parses only appended bytes; a trailing line without its newline is left for the next fold.

[[src-tauri/src/live_tracker.rs#LiveTracker#sweep]] is the cold start, watcher-overflow recovery, and periodic backstop for all three providers. It uses [[src-tauri/src/sessions.rs#discover_claude_transcripts_in]], [[src-tauri/src/sessions.rs#discover_codex_transcripts_in]], and [[src-tauri/src/sessions.rs#discover_pi_transcripts_in]], then releases any session silent past [[src-tauri/src/live_tracker.rs#IDLE_AFTER]]. [[src-tauri/src/live_tracker.rs#modified_within_idle_window]] gates Claude and Pi transcript reads on modification time; Codex still indexes quiet ancestors before applying the same parse gate.

A state-changing fold emits `sessions-live-updated`. The emit carries no window of its own: the watcher drains its pending set at most once per quiet window and the frontend throttles the fan-out those events wake, so a burst is already coalesced on both sides of this one.

[[src-tauri/src/live_tracker.rs#LiveTracker#session_ranking_keys]] and [[src-tauri/src/live_tracker.rs#LiveTracker#overlay]] are the Sessions read path's only live-state surface. The keys enter the retained query so a folded session survives storage's provisional limit and can compete for final ranking; the overlay then gives each covered row its open agents and, when the fold is newer than the retained evidence, its liveness, synthesizes an observed-only row for a folded session with a validated root cwd that storage has no row for, and re-ranks the merged set before truncating it to the same clamped limit storage applies. An agent's model comes from the fold rather than from retained child evidence, so the read resolves a label in one pass and schedules no rescan. Rows the fold does not cover — every remote host — keep null agent fields, and terminal projection and the runtime pass run afterwards unchanged.

[[src-tauri/src/transcript_watcher.rs#admit_pending]] feeds Claude/Codex/Pi transcript evidence. Watcher batches go to the fold alongside retained ingest, while authenticated Pi lifecycle pushes mutate the same map after durable lifecycle persistence. Startup manages the tracker before either feed starts and applies saved activity-tracking and per-provider switches, so a disabled provider is never folded or lifecycle-created.

### Claude Fold Rules

Claude transcripts fold by the role their own path states: the session's transcript, a sub-agent transcript at any depth, or a workflow journal.

The session transcript's first timestamped record supplies origin and project through [[src-tauri/src/live_tracker.rs#claude_session_origin]] and is never re-read, so the origin is established once. Timestamped non-`attachment` records from anywhere in the tree advance activity, because Claude writes hook results after SessionEnd and that bookkeeping must not reopen a finished session.

A sub-agent's `.meta.json`, read by [[src-tauri/src/live_tracker.rs#read_agent_meta]], supplies its spawning `toolUseId` and agent type. It is written beside the transcript at spawn and can lose the race to it, so the read is retried on every later event for that agent until it lands rather than once when the file is first seen. [[src-tauri/src/live_tracker.rs#LiveSession#agent_open]] then keeps a tool-spawned agent open until that id appears in a `tool_result` — one resolved-id set per session, fed by every file in the tree, which is what covers the depth≥2 spawn whose result lands in the parent agent's transcript rather than the root. A workflow agent under `subagents/workflows/wf_*/` answers instead to its `journal.jsonl`, which the fold pulls in alongside the agent because the transcript walker enumerates agent files but not journals. An unresolved spawn whose own transcript went silent past [[src-tauri/src/live_tracker.rs#IDLE_AFTER]] is abandoned rather than slow.

A backgrounded spawn answers that same tool call, but its `tool_result` is only the launch receipt Claude Code writes within a second of starting it. [[src-tauri/src/live_tracker.rs#tool_result_ids]] therefore drops the block [[src-tauri/src/live_tracker.rs#is_async_launch_receipt]] identifies, and [[src-tauri/src/live_tracker.rs#task_notification_tool_use_id]] resolves the spawn from the `<task-notification>` the harness delivers when the agent actually ends. Without that split every async agent closed on arrival and no live agent rail ever rendered.

[[src-tauri/src/live_tracker.rs#claude_record_model]] takes an agent's model from its own assistant records, validated through the same gate retained evidence passes, so a sub-agent's label needs no retained child evidence and no second scan to resolve.

### Codex Fold Rules

A Codex rollout's path states no role, so the fold takes identity from the rollout's own head record and keeps it for the life of the thread.

[[src-tauri/src/live_tracker.rs#TrackerState#codex_file]] keys every enumerated rollout by [[src-tauri/src/sessions.rs#codex_thread_id]] into an incremental thread index. The index covers the whole corpus rather than the live window, because a live spawn routinely names an ancestor that has been quiet for hours, and a batch is indexed in full before any of it is resolved so a child folded ahead of its parent still finds a chain to walk. [[src-tauri/src/live_tracker.rs#codex_head]] then parses `session_meta` once per thread through [[src-tauri/src/transcript_identity.rs#codex_metadata]] — the parser retained ingest uses — and [[src-tauri/src/live_tracker.rs#codex_root]] walks `parent_thread_id` / `forked_from_id` to the user thread that owns the spawn. Both answers are permanent state, so every later event on that rollout costs map lookups. A rollout nothing has written to for a whole [[src-tauri/src/live_tracker.rs#IDLE_AFTER]] window is indexed but not opened: it can hold neither a live session nor an open agent, and cold-parsing the whole corpus instead would cost gigabytes of bounded reads at startup.

First sight of a rollout, and any rewrite of one, parses from a bounded tail through [[src-tauri/src/live_tracker.rs#read_codex_tail]] rather than from the head — a spawned rollout reaches hundreds of megabytes. Thereafter only appended bytes are parsed. A root rollout's head supplies the session's origin and project and the floor its activity falls back to, so a rewrite clears what the replaced file had contributed and its own tail answers instead.

[[src-tauri/src/live_tracker.rs#codex_turn_boundary]] flips a sub-agent's open bit: the newest `task_started` / `task_complete` / `turn_aborted` in its own rollout, never a count, because a sub-agent thread outlives one turn and starts exceed completes plus aborts in most multi-turn rollouts. A tail window holding no boundary is itself the answer — a rollout only accumulates records inside a turn — so a cold start seeds the bit from whether the window reached the start of the file. Silence past the idle window still closes an agent whose turn never ended, and the boundary's own timestamp feeds that clock because a turn can pass without a substantive record. [[src-tauri/src/live_tracker.rs#codex_activity_timestamp]] advances session activity from substantive user, assistant, reasoning, and tool content only, so post-Stop lifecycle, token bookkeeping, empty items, and the file's mtime cannot reopen a finished session. The model is the first one a rollout's `turn_context` records name, read by [[src-tauri/src/live_tracker.rs#read_codex_model]] from the head parse's own handle.

### Pi Fold Rules

A Pi session file states its own identity, so the fold takes it from the header record rather than from the path.

[[src-tauri/src/live_tracker.rs#TrackerState#pi_file]] reads that header through [[src-tauri/src/pi_session.rs#read_pi_session_header]] — the same bounded v2/v3 probe notify validation uses — and remembers the answer on the file's own tail entry, so a warm sweep re-reads no headers. The header supplies the session's origin and project and the floor its activity falls back to before the first prompt is typed.

[[src-tauri/src/live_tracker.rs#fold_pi_line]] then folds only appended bytes. Substantive `user`, `assistant`, and `toolResult` entries advance activity, so custom extension records, `quill-tracking` lifecycle entries, thinking-level, and compaction markers cannot reopen a finished session. An assistant message names the model answering and its `usage.totalTokens`; a `model_change` names a switch no message has answered under yet. A nested child's own `session_info` names its role only when its label repeats the run id the structural path states. Pi writes no end record, so closure is the shared idle cutoff, the same one Claude and Codex answer to.

A rewritten file is shorter than the offset already consumed, which clears the activity and cumulative total it had contributed before the replacement is folded whole. `process_instance_id` and `recovering` exist only for protocol-2 lifecycle state, so a folded session claims neither.

### Pi Lifecycle Rules

Protocol-2 lifecycle preserves remote identities and lineage where disk-folding cannot reach.

Local session files own live activity, models, tokens, agents, tools, skills, search, and response times. Durable `pi_session_lifecycle` and `pi_event_receipts` remain because remote-host lifecycle, process ordering, event idempotency, and transactional lineage have no local file sweep to reconstruct them from.

[[src-tauri/src/transcript_analytics.rs#source_local_response_times]] derives each Pi turn from persisted user and assistant records under the canonical source key; no runtime-message push is needed. The extension calls Session Search notify only when Pi supplies a transcript path already on disk. Startup inventory, the filesystem watcher, and the 120-second rescan recover the same persisted source without notify. Validation uses [[src-tauri/src/pi_session.rs#read_pi_session_header]] as a bounded v2/v3 probe, then shared parsing indexes messages and schedules authoritative reconciliation. A missing path provides no persisted evidence to invent.

### Pi Session Lineage

Pi's runtime-owned nested layout, `<timestamp>_<parent-uuid>/<run-id>/run-N/session.jsonl`, is structural direct-agent proof: the enclosing directory is written by Pi and names the parent header id.

[[src-tauri/src/live_tracker.rs#pi_path_lineage]] accepts only that exact timestamp, UUID, run-index, and `session.jsonl` shape; timing, cwd, arbitrary filenames, models, and merely similar directories remain forbidden evidence. The child header still names its own identity. Its `session_info` label supplies an agent role only when it repeats that same runtime run id. This is a runtime tree fact, not an inference rule relaxation.

[[src-tauri/src/live_tracker.rs#resolve_pi_root]] receives structural edges through its existing bounded resolver. Structural agent edges are primary over a conflicting push; pushed lifecycle lineage and role corroborate without creating another projection. A flat Pi session has no structural edge, stays an independent root row without explicit proof, and retains the existing push path unchanged. In particular, this fold deliberately leaves a flat `PI_SUBAGENT_PARENT_SESSION` child unresolved; only its already-existing launcher lifecycle proof may attach it. Provider and host remain part of the live key, so equal ids from other providers or machines cannot join.

[[src-tauri/src/live_tracker.rs#LiveTracker#overlay]] derives relationships from the bounded session map on each read. Generic linked children remain independent rows and feed the parent's linked-session rail. Explicit agent children are omitted as rows and feed the root's agent count, active runtime, and open-agent rail; while any of them stays open, the root's activity is the overlay's own read instant rather than the newest transcript flush, so a Stop-derived terminal recorded at the root's turn settle cannot outrank a rail whose agents are still working. Provider disable, activity disable, and idle eviction remove the source sessions and their relationships.

`subagent-artifacts/<run-id>_<agent>_<child-index>_transcript.jsonl` is explicitly unsupported for this rail. It is a pi-subagents-owned version-1 artifact schema, not a Pi session file, and states no parent session id; LiveTracker rejects it at the required Pi session header rather than guessing from its name or fields. Session notify carries the same pushed proof captured at start, including post-turn refreshes, and Search stores generic and agent parent ids in each Pi document. Root and unresolved proof clear parser fallback. Sessions navigate to the provider-qualified parent without exposing paths.

## Model Observation Reconciliation

Retained Claude and Codex transcripts become source-owned model observations without coupling model identity to Session Search indexing.

[[src-tauri/src/sessions.rs#enumerate_retained_jsonl_source_roots]] canonicalizes each configured provider root and contained supported JSONL path. [[src-tauri/src/sessions.rs#canonical_source_key]] combines Claude/Codex root identity with native path bytes; Pi instead derives its canonical owner from the bounded session header plus the normalized local hostname. Full inventory and [[src-tauri/src/sessions.rs#validate_retained_notify_source|live notify validation]] therefore address the same source without lossy path or cross-provider collisions.

[[src-tauri/src/model_usage.rs#parse_claude_model_usage_jsonl]] emits Claude assistant turns from explicit `message.model` plus any dimensions on that record. [[src-tauri/src/model_usage.rs#parse_codex_model_usage_jsonl]] keeps explicit `turn_context` model evidence separate from normalized cumulative `token_count` deltas. Missing or invalid identity stays null on the raw turn record, preserving replayable evidence without inventing an `unknown` model. After adapter parsing, [[src-tauri/src/model_usage.rs#apply_carry_forward_attribution]] stamps each observation's `derived_model_id` with its chain's running model — the last non-null, non-`<synthetic>` raw id, which synthetic turns never update — so within-chain token rows inherit attribution and only rows before any model evidence stay unattributed; aggregation keys on the derived id while segment and switch semantics stay on raw turn evidence. Forked Codex subagent rollouts declare their parent through `parent_thread_id` or `forked_from_id` and embed `session_meta` copies restating the whole ancestor chain; [[src-tauri/src/transcript_identity.rs#resolve_codex_native_identity]] walks that declared chain at any depth, keeps the child identity, and marks a source conflicted only for identity claims outside the chain, which alone exclude it from analytics.

Persisted Pi assistant usage owns one canonical host-qualified source through [[src-tauri/src/storage.rs#pi_source_key]]. Reconciliation replaces that source's unpruned observations, token snapshots, rollups, and model registry in the same transaction as lifecycle/runtime/tool evidence. Migration 44 owns earlier source-less runtime, migration 45 compacts live state, and migration 46 rekeys both domains by normalized host plus session after a verified schema-45 backup. Insert-time watermark filtering prevents replay from restoring pruned raw rows. Sessions and model history report spend truth with [[src-tauri/src/models.rs#ModelTokenScope]] fixed to `all-branches`. Test coverage lives in [[pi-model-usage-tests#Pi Model Usage Test Specs]].

[[src-tauri/src/model_usage.rs#prepare_model_source_reconciliation]] stages complete source reads, content hashes, provider parsing, and graph resolution into an owned plan before any replacement transaction. Both analytics domains use [[src-tauri/src/transcript_identity.rs#read_stable_transcript]] for the bounded read: it retries path or open-handle changes, rejects oversized files, and hashes exactly the stable bytes each parser consumes. Filesystem layout hints can report conflicts but cannot override transcript-native parent metadata.

`prepare_model_source_reconciliation` enumerates and fingerprints the entire affected root, so it is reserved for the full backfill pass. The live queue instead calls [[src-tauri/src/model_usage.rs#prepare_scoped_model_source_reconciliation]], which loads each affected root's persisted source inventory with one indexed query — no filesystem walk — and only reads, hashes, and parses the queued changed sources. [[src-tauri/src/model_usage.rs#build_scoped_source_root_graph]] seeds the resolution graph from those staged sources plus every persisted sibling's stored chain metadata, and [[src-tauri/src/model_usage.rs#stabilize_scoped_root_graph]] re-parses only descendants whose resolved analytics root actually moves, so editing one transcript no longer restages the whole tree while cross-source chain and subagent resolution stay correct. The live plan captures no prune proofs.

When an ancestor changes a retained descendant's resolved analytics root, preparation reparses that otherwise unchanged descendant before writes begin. [[src-tauri/src/model_usage.rs#commit_next_model_source_batch]] commits bounded source batches from the stable plan so a worker can yield without losing graph context; errors return prior committed outcomes. Preparation captures prune proofs only for roots complete in that inventory, and pruning also requires every planned source commit. Event status is read before mutation, each source commit remains atomic, and post-commit `model-analytics-updated` delivery is best-effort and storage-free.

[[src-tauri/src/storage.rs#Storage#replace_model_source]] folds each retained replacement batch into [[backend#Backend#Database#Schema#Hourly Analytics Rollups|model_usage_hourly]] with one SQLite grouped insert inside the same immediate transaction as raw observation and source metadata writes. It replaces the source's unpruned rows, ignores conflicts with authoritative pruned rows, then advances `rollup_generation` once when rollup rows changed; any later failure rolls raw evidence, rollups, source metadata, and generation back together.

The first model-rollup pass and manual rebuild share [[src-tauri/src/rollup_backfill.rs#run_rollup_backfill]]. A committed bookmark names the last complete UTC hour; each next transaction replaces current raw-backed groups across a half-open hour range, preserves pruned authority, and yields before its WAL checkpoint. Existing raw reads remain active until `model_backfill_status` commits `complete`.

Deletion keeps each removed source fingerprint as durable suppression and removes that source's raw-backed and pruned hourly rows in the same transaction. Reconciliation skips unchanged suppressed content; only one successful atomic replacement of changed content clears suppression and restores observations. Re-ingest refolds unpruned groups but never updates an authoritative `raw_pruned=1` conflict. Overview, paging, and chain queries join active source ownership at read time, so suppression and restored visibility follow one lifecycle without rewriting rollups.

The [[src-tauri/src/storage.rs#Storage#get_model_usage_overview|overview]] and [[src-tauri/src/storage.rs#Storage#get_model_sessions|paged-session]] reads use separate read-only deferred transactions rather than the primary connection mutex. Once rollup backfill completes, overview reads closed UTC hours from `model_usage_hourly` and raw evidence for partial hours and documented facet exceptions. Incomplete states preserve the raw path and expose `buildingIndex`.

[[src-tauri/src/lib.rs#enqueue_retained_live_source]] admits validated notifications to one coordinator keyed by provider and canonical source key. Each entry tracks independent model and transcript pending, running revision, failure count, and ready time. A newer Claude/Codex notification rearms both domains; Pi rearms transcript replacement only, so native usage is not parsed through a second model job. Older completion cannot clear newer work, and one domain can back off while siblings continue. [[src-tauri/src/lib.rs#drain_model_usage_live_queue]] still owns the model permit and commit events.

[[src-tauri/src/lib.rs#spawn_startup_model_source_reconciliation]] re-admits Claude/Codex inventory to the model side after runner state initializes, including when the one-time backfill is already complete. Pi startup recovery is owned by the transcript reconciliation pass. Startup, notify, and rescan admissions converge on the same canonical source entry without duplicate Pi model work.

After storage initializes, [[src-tauri/src/lib.rs#run]] resets interrupted running history to pending and reserves one nonblocking migration/resume worker. Explicit [[src-tauri/src/lib.rs#retry_model_history_backfill]] uses the same reservation before changing durable state, so concurrent retries are idempotent and an unowned persisted `running` row is safely recovered; live work can finish under the shared permit before the pending retained pass starts.

[[src-tauri/src/sessions.rs#SessionIndex#startup_scan]] scans retained Claude, Codex, and Pi roots through their concrete walkers. Pi identity comes from its bounded native header plus local normalized hostname, so startup search and analytics inventory address the same source without waiting for `sessions/notify`.

[[src-tauri/src/transcript_watcher.rs#start]] recursively watches distinct Claude, Codex, and Pi transcript roots. It admits debounced JSONL changes through strict validation. Missing or failed watches retry every 120 seconds, and ambiguous canonical roots are rejected.

Nonempty watcher batches, recovery events, and newly attached roots rerun incremental Session Search and admit changed sources to the shared coordinator. Pi arms transcript work only; Claude and Codex arm both transcript and model domains.

[[src/hooks/useModelAnalytics.ts#useModelAnalytics]] requests only one command-and-arguments-scoped overview through the process-lifetime invoke cache. Data-changing model events invalidate that key and join the shared five-second-or-longer mounted fan-out; the 60-second fallback poll follows the same path. Each accepted overview advances the frontend refresh generation and updates both [[src/components/widget/views/ModelsView.tsx#ModelsView]] bands as one unit. The widget has no selected-model paging or lazy chain-history request to fan out.

## Session Indexing Pipeline

Session transcripts are indexed for full-text search with enriched metadata, while provider-aware side tables keep tool and latency data distinct.

1. Claude Code writes session JSONL files to `~/.claude/projects/`, Codex writes rollout transcripts to `~/.codex/sessions/`, and Pi writes tree sessions to its configured session directory
2. When Session Search opens, [[src-tauri/src/sessions.rs#SessionIndex#startup_scan]] scans Claude, Codex, and Pi incrementally by mtime
3. Provider hooks post `POST /api/v1/sessions/notify` with a JSONL path and provider metadata; Pi also pushes its stable session identity and lineage proof, while remote sync can push `POST /api/v1/sessions/messages`
4. `notify` requests acknowledge first, then feed independent search and analytics schedulers; remote `messages` requests acknowledge only after analytics commits, while Tantivy indexing remains asynchronous and best effort. Pi messages use a session-owned live source; other remote providers remain source-less.
5. Local Claude full-transcript sync runs on `Stop`, `StopFailure`, and `SessionEnd` instead of every `PostToolUse`, so full-file reindexing happens only at terminal boundaries
6. Provider-specific parsers enrich messages: Claude tool blocks and Codex function/custom tool calls become tools_used, files_modified, code_changes, commands_run, and tool details. Pi indexes user and assistant entries once by id, converts assistant `toolCall` blocks into the same metadata, and attaches each matching `toolResult` as a 10 KiB `full_output` plus a 300-byte command preview.
7. Tantivy stores provider, message_id, session_id, content, role, project, project_path, host, timestamp, git_branch, and enriched metadata. Pi result/custom roles are excluded; Claude and Codex provider-native search roles remain visible. A one-time provider-scoped cleanup removes legacy Pi non-conversation documents without rebuilding notify-only history.
8. Retained runtime, response, tool, skill, hook, Pi lifecycle/receipt, and Pi native-usage rows persist in provider-aware SQLite snapshots owned by canonical source. Pi persisted sources use that same host-qualified owner; other remote message and hook pushes keep source-less identities.
9. Search uses boosted BM25 scoring and snippet generation; concrete file and tool fields outrank prose.
10. Faceted search pre-aggregates provider, project, and host counts.
11. Desktop callers keep the full search response. MCP and Pi request `view=compact`, which returns snippet and identity fields without full content and stops before 32 KiB; Pi repeats that bound for older servers and stores no duplicate payload in `details`.

### Enrichment

Each message is enriched during indexing by parsing tool call inputs and outputs.

Claude `Edit`, `Write`, `MultiEdit`, and `NotebookEdit` tool calls become `code_changes`, Bash becomes `commands_run`, and Read/Grep/Glob become `tool_details`. Codex `apply_patch` (as either a custom-tool call or a function call), `exec_command`, and `write_stdin` map to `code_changes`/`commands_run` respectively, and MCP or auxiliary tool calls become searchable `tool_details` — except MCP tools whose input carries a clear file-write shape (`old_string`/`new_string`, or `file_path`/`path` plus `content`), which are classified `code_changes`.

Pi maps lowercase `write` and `edit` to code changes, `bash` to commands, and `read`/`grep`/`find`/`ls` plus custom tools to tool details. Write and edit inputs retain full-input line counts before the 10 KiB stored-input cap. Tool-result text is correlated by `toolCallId`, capped at 10 KiB, and excluded as a standalone message; non-user/assistant Pi roles are also excluded. Skill attribution reads the same lowercase names: `read` resolves a SKILL.md path exactly as Claude's `Read` does and `bash` matches Codex's `exec_command`, which is the only way a Pi skill is ever observed because Pi has no `Skill` tool to declare one.

Codex synthetic tool messages keep the original call timestamp when later output is attached. This preserves chronological Tantivy ordering and transcript response-time pairing while retaining the output content.

For every `code_change` action, per-action `lines_added`/`lines_removed` are computed here from the FULL, untruncated tool input before `full_input` is capped at 10KB, then stored in the `tool_actions` columns of the same names (migration 33). This avoids the prior undercount where large edits truncated past 10KB failed to re-parse and counted zero. MultiEdit sums line counts across its `edits`, NotebookEdit counts `new_source` lines (removed for `delete` mode, added otherwise), and apply_patch counts `+`/`-` patch lines.

### Dual Emission for Runtime Tracking

The same parse pass that produces `ExtractedMessage` for the search index also produces `ExtractedEvent` for the [[backend#Database#Schema#Code and Runtime Metrics]] `session_events` table.

The search index drops Claude `tool_result`-only user messages and empty assistant blocks, admits only user/assistant roles for Pi, and preserves intentional Claude/Codex provider-native roles. The event stream still carries every non-meta `user`/`assistant` line with a non-empty timestamp — classified by content shape into `user_text`, `user_tool_result`, `asst_text`, `asst_thinking`, or `asst_tool_use`. Retained-source reconciliation owns transcript analytics persistence; Tantivy startup and `/sessions/notify` indexing never delete or insert those rows. The `/sessions/messages` remote-push handler stores source-less events through [[src-tauri/src/storage.rs#Storage#store_live_session_analytics]], consuming ordered event kinds when supplied and retaining the one-event `(role, content, tools_used)` heuristic for older clients.

Claude remote sync keeps one wire message per timestamped provider record while sending every runtime role in canonical order: user tool result then text, or assistant thinking then text then tool use. This retains mixed-block semantics without duplicating response-time messages. Text and tool names remain flattened for search, while a narrow `assistant_tool_use` type hint supports older servers. Native `sessionId`, `isSidechain`, `agentId`, and `parentUuid` fields cross the wire explicitly; incomplete native child identity is never rewritten as parent activity. Native and fallback IDs use disjoint `claude:native:` and `claude:fallback:` namespaces; UUID-less rows derive stable identity from a root-session-plus-source hash and absolute source-line ordinal.

Pi pushes stable turn and tool message ids through the same endpoint. Turn/input and turn-end map to user/assistant response boundaries; tool execution maps to the canonical tool-use/result pair, and Pi never synthesizes an `asst_thinking` event because that evidence is unavailable.

Codex inter-agent `response_item.agent_message` records are search-only messages: text blocks are flattened, sender and recipient stay visible in content, and the sender remains the role instead of being rewritten as user or assistant. Lifecycle/status events, tool-end notifications, `turn_context`, `world_state`, `compacted`, and inter-agent metadata stay out of search; dedicated model, runtime, and tool branches retain the evidence they own without duplicating tool output.

Each Claude sync fire sends successive chunks of at most 500 messages under one shared 8-second, 18-request budget, keeping a normal user/assistant pair on the same side of a chunk boundary. Every accepted or deliberately dropped contiguous segment checkpoints the next unsent absolute source line, so skipped lines do not replay and a partial failure retries only remaining messages. Row-level 400 responses may bisect a chunk to isolate poison records; envelope rejection, timeout, budget exhaustion, and transport failure never advance the unaccepted range.

`splitSourceLines` reports whether the transcript ended on a newline, and a trailing unterminated line is never acknowledgeable. Without that, a cursor could jump past a record the provider was still writing, and the completed record would be lost silently and permanently. `postJSON` likewise separates permanent failures (4xx other than 408 and 429) from retryable ones. A row-attributable `400` is bisected down to the single poison record, which is logged and dropped so the rest of the session syncs; envelope-level `400`s and `401`/`403`/`404`/`413` hold the cursor instead, because no amount of bisecting can isolate an envelope problem. Previously any `400` wedged a session's sync forever with no signal. Identity guards log unconditionally and send the homogeneous prefix rather than discarding the whole batch, and client-side pre-filters mirror the server's own validation (RFC3339 timestamps, `MAX_CONTENT_LEN`, field length caps) so avoidable rejections never reach the wire. Cursor writes use an owner-specific temporary file, fsync and close it, then rename it over the prior cursor so interruption cannot expose an empty or prefix value. The next exclusive lease owner removes crash-orphaned cursor temps for that source before resuming.

A stable per-transcript-source lease root contains random owner-specific candidate directories, so a parent and its sub-agent files never share a cursor or lock. Acquisition scans before and after candidate creation; concurrent candidates either leave one winner or all back off. Owners heartbeat and remove only their own inode-and-token-qualified path, so an expired owner's cleanup cannot address a replacement path. Stale candidates are likewise pruned only by their unique path.

Feature 009 adds a third sibling to this dual emission: the same Claude JSONL walk peels off `type:"attachment"` records whose `attachment.type` begins `hook_` (e.g. `hook_success`, `hook_failure`, `hook_timeout`, `hook_blocked`) via [[src-tauri/src/sessions.rs#extract_hook_invocation_from_attachment]], producing one [[backend#Database#Schema#Hook Invocations]] row per fire. Sub-agent transcripts inherit the `is_sidechain=1` and `agent_id` columns automatically because the attachment extractor reads the same record-level fields the message extractor does. Codex rollouts do not emit attachment records, so Codex hook telemetry instead arrives live via `POST /api/v1/hooks/observed` from a deployed observer script.

### Source-Owned Analytics Snapshots

Retained-source reconciliation runs in two phases so peak memory is one source rather than the entire retained corpus.

[[src-tauri/src/transcript_analytics.rs#reconcile_transcript_source_root]] phase one resolves cross-source native identity only, retaining a few hundred bytes per source and dropping every record and byte it read, so the whole-root graph is known before the first commit. Phase two parses, stamps, commits, and drops one snapshot at a time. The cost is a second read of each source that actually needs committing; the prior design held every snapshot for both roots resident simultaneously.

[[src-tauri/src/storage.rs#Storage#replace_transcript_analytics_snapshot]] replaces one source's raw analytics and runtime fold atomically. After retained events are persisted, [[src-tauri/src/storage.rs#refold_runtime_source]] rebuilds only that source's unpruned hourly runtime and open-turn state from persisted timestamps. Closed logical turns therefore stay independent of wall clock and source re-ingest cannot double-count them.

Before runtime backfill completes, [[src-tauri/src/storage.rs#Storage#get_llm_runtime_stats]] uses its bounded raw-event path. After completion, it reads finalized turns from [[backend#Backend#Database#Schema#Hourly Analytics Rollups|runtime_hourly]] and active Pi open tails from one state row per source; transcript tails still seek after their bookmark. Closed duration and start-hour attribution are pure functions of persisted events; only a trailing open tool wait consults the pinned query clock, capped at six hours, while an ordinary open turn ends at its last event.

[[src-tauri/src/transcript_identity.rs#resolve_codex_native_identity]] preserves the first Codex child identity while accepting consistent ancestor restatements and rejecting conflicts or cycles. [[src-tauri/src/transcript_analytics.rs#resolve_claude_native_identity]] skips anomalous Claude records into bounded diagnostics. [[src-tauri/src/transcript_identity.rs#resolve_pi_native_identity]] derives Pi's native identity from the session header; exact `quill-tracking` entries then supply process lifecycle, direct lineage, role, and event receipts, while native assistant messages supply usage. [[src-tauri/src/transcript_analytics.rs#owned_tool_rows]] remains the one tool/skill builder for retained and notify paths, so action keys and skill fan-out cannot drift. Notify still performs an immediate two-table replacement as a search-independent fast path, then the shared coordinator authoritatively replaces Pi runtime, response, tool, skill, lifecycle, receipt, token, usage, rollup, and registry rows under the same source key. [[src-tauri/src/transcript_analytics.rs#parse_transcript_analytics_source]] performs one bounded stable read with original line ordinals, and [[src-tauri/src/transcript_analytics.rs#stamp_analytics_root]] validates native attribution before the atomic commit.

Because the two phases read at different times, [[src-tauri/src/transcript_analytics.rs#native_identity_matches]] re-checks the parsed identity against the inventoried one before stamping. A file that changed in between would otherwise be stamped with a stale root and silently reparented, so drift is a source failure that retains last-known-good rows. `cwd` is excluded from that comparison as descriptive origin, so a moved checkout is not drift.

Startup reconciliation allocates durable per-root generations, resolves each provider-qualified native chain independently, and resumes the migration-armed historical rebuild until all available roots finish. While the reingest marker is set, `force_full_reparse` bypasses both the mtime/size fast path and the content-digest short-circuit; suppression is honoured regardless and never bypassed. Unchanged sources owe only a `seen_generation` bump, flushed for the whole root in one transaction. Unrelated sessions sharing one provider directory retain distinct roots, and Tantivy indexing performs no analytics writes.

Failure is isolated per source. `RootReconciliationFault` separates a source-scoped failure from a `RootUnavailable` root that simply produces no prune proof and from a `Database` fault — failure of the bounded diagnostic upsert itself — which is the only one that abandons the run, because after it nothing can retain last-known-good state. For Pi, the owning failure call also marks the reporter subject associated through `pi_session_lifecycle` as `reconciliation_failed`; committed persisted-source recovery clears that same process's source dimension. Reconciliation replaces only source-owned rows, and pruning is gated on enumeration completeness alone: a failed source cannot block it, because [[src-tauri/src/storage.rs#Storage#record_transcript_analytics_source_failure]] refreshes `seen_generation`, so the `seen_generation < ?` prune query can never select it. [[src-tauri/src/storage.rs#Storage#prune_transcript_analytics_sources_for_root]] surfaces a row-decode failure while collecting prune keys instead of swallowing it, so a partial key set cannot look like a complete one.

Live notifications use the shared provider-plus-source coordinator while transcript reconciliation keeps its own completion and retry state. Scoped reconciliation combines the changed source with persisted sibling identities and reparses only descendants whose resolved root moves. A provider/root permit serializes full inventory-through-prune and scoped prepare-through-commit lifecycles, while registry writes reject older generations.

Live coverage no longer depends solely on the per-session `/sessions/notify` hook. [[src-tauri/src/transcript_watcher.rs#start]] coalesces relevant JSONL bursts for at most one second, then reuses strict source validation and the canonical live queue. Remove, rename, notify-rescan, and bounded-buffer overflow signals promptly invoke existing whole-root graph reconciliation and pruning. The 120-second rescan loop ([[src-tauri/src/lib.rs#spawn_transcript_rescan_loop]], see [[architecture#Background Tasks]]) reuses one inventory for whole-root reconciliation and changed-source admission, recovering missed deletions and events. Unchanged sources short-circuit to a stat-only `SuppressedUnchanged` verdict.

Per-source capped backoff lets healthy siblings continue after one source fails. A successful changed snapshot emits `transcript-analytics-updated`, refreshing runtime and breakdown views without relying on Session Search events.

#### Live Source Coordinator Test Specs

These tests pin independent domain scheduling on one canonical source entry.

##### Independent Domain Retry

A committed model result must remain complete when transcript reconciliation fails and enters capped backoff.

##### Newer Notification Wins

A notification arriving during an older attempt must rearm both domains, and the older completion must not clear that work.

##### Healthy Sibling Progress

One failed transcript source must not delay a healthy sibling or its independent model work.

##### Model Backfill Isolation

A reserved model-history backfill must gate model work only; transcript jobs remain immediately eligible.

#### Transcript Watcher Test Specs

These tests pin provider routing, bounded coalescing, event filtering, and recovery behavior without starting a live Quill window.

##### Provider Paths And Burst Coalescing

Claude, Codex, and Pi JSONL paths must retain their provider while duplicate bursts collapse to one pending path and flush by the one-second ceiling.

##### Relevant Event Filtering And Prune Recovery

Only unambiguous in-root JSONL create, content-write, and rename targets may enter targeted admission. Remove and rename events must request whole-root pruning recovery; metadata stays ignored.

##### Late Root And Watch Recovery

A missing or failed provider root must retry registration later, while an already watched root must never register twice.

##### Bounded Overflow Recovery

Exceeding either bounded event buffer must request prompt whole-root reconciliation instead of silently waiting for periodic recovery.

##### Duplicate Root Rejection

Two providers resolving to one canonical root must not register the later duplicate or route either provider's event by first match.

##### Live Tracker Admission

A filesystem event on a watched transcript must reach the live tracker in the shape admission drains it, folding that session without a Quill window or an app handle.

### Live Analytics Origin

Source-less analytics retain only origin fields explicitly supplied by their live producer, so ownership is always recorded rather than guessed from row-local data.

[[src-tauri/src/storage.rs#Storage#store_live_session_analytics]] commits `/sessions/messages` runtime rows with project, full cwd, hostname, and native chain identity, or `/hooks/observed` hook rows with cwd, beside one `live_analytics_sessions` mapping. The optional cwd and chain wire fields preserve older clients. Later writes merge non-null origin fields.

The HTTP handler validates every flattened message before persistence: UUIDs are trimmed, non-empty, and unique; roles are `user` or `assistant`; timestamps parse as RFC3339; explicit child rows require consistent root, chain, parent, and agent IDs; supplied event kinds must be a canonical role-specific subsequence. Any malformed row rejects the whole batch with `400 Bad Request`. Message UUID plus explicit per-message event ordinal provides stable live event identity, while response timing still consumes one original message row. Storage repeats identity and contiguous-ordinal checks inside one transaction. A `2xx` response means that transaction committed, so the bridge may advance its durable cursor; storage failure returns `500`, while missing or failed Tantivy indexing cannot discard committed analytics.

Raw candidate paths retry only while ownership validation is unavailable; invalid candidates are dropped. A validated canonical source enters the shared coordinator in one non-fallible state mutation after managed state exists, so admission cannot partially accept one analytics domain. Session Search availability remains independent.

### Sub-Agent Transcripts

The shared Claude walker covers flat parents and the full `subagents/` subtree for strict retained inventory and permissive Session Search.

It collects every `.jsonl` at any depth: flat `subagents/agent-*.jsonl` plus Workflow-spawned agents nested one level deeper at `subagents/workflows/wf_<id>/agent-*.jsonl` (~20% of agents on a heavy Workflow user). The walk is bounded to that subtree so unrelated nested JSONLs never sneak in.

Each sub-agent file becomes a separate ingest entry. Claude uses `agentId` as native child chain and `sessionId` as parent; Session Search indexes the child chain so sibling transcripts never replace each other and context opens the matching source. Codex forked rollouts use their first child `session_meta.id` and `parent_thread_id` or `forked_from_id`. Resolved-root `session_id` is stamped later without replacing either provider's child identity. Measured across all Workflow-nested agent transcripts, only `promptId` is ever missing from the first record (absent in ~2% of files); `cwd`, `entrypoint`, `gitBranch`, and `version` are always present. Identity resolution still reads every non-linkage field as optional and never assumes those keys exist, as a defensive stance rather than one required by observed data.

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

## Usage Bucket Fetching

The main window polls configured CPA or, when CPA is absent, enabled native providers for live rate-limit status, then stores source-aware results in shared usage tables.

1. `fetch_usage_data()` resolves the enabled provider list from the integration manager, then suppresses the entire native list when a CPA connection is configured
2. Claude polling in [[src-tauri/src/fetcher.rs]] calls the Anthropic API with an OAuth Bearer token and parses Claude bucket keys
2a. Before Claude makes a live request, [[src-tauri/src/lib.rs]] reuses the most recent persisted `usage_snapshots` rows when they are newer than the 3-minute live refresh interval, so window reopens and app restarts do not immediately hit the Anthropic endpoint again
3. Codex polling in [[src-tauri/src/fetcher.rs]] first calls `codex app-server` over stdio and requests `account/rateLimits/read`, which returns a multi-bucket `rateLimitsByLimitId` view that includes the base Codex limits plus model-specific limits such as Codex Spark
4. The fetcher skips unrelated stdio frames like the `initialize` response and only parses the app-server message whose request id matches the rate-limit call
5. Each bucket is normalized to `{ provider, key, label, utilization, resets_at }` and validated for finite utilization plus RFC3339 reset timestamps
5a. Each Codex rate-limit snapshot may also carry a `credits` object (`balance`, `hasCredits`, `unlimited`). The fetcher extracts the first non-null, non-unlimited credit balance and returns it as a `ProviderCredits` entry alongside the buckets
6. If the direct Codex app-server request fails, [[src-tauri/src/fetcher.rs]] falls back to the newest `token_count` transcript event in `~/.codex/sessions/**/*.jsonl` so older Codex installs can still populate base usage rows
7. Successful live buckets are inserted into `usage_snapshots`, keyed by provider plus bucket key, and hourly cleanup aggregates them into `usage_hourly`
8. If a provider poll fails, the command loads the last stored buckets for that provider and returns a provider-scoped error alongside the cached rows
8a. Claude 429 responses persist a rate-limit cooldown timestamp in the settings store, and subsequent refreshes honor that cooldown before retrying the live API. While the cooldown holds the poll serves the last persisted snapshot and pushes `ProviderErrorKind::Stale` via [[src-tauri/src/lib.rs#push_stale_error]] — both on the fresh 429 that arms the cooldown and on every subsequent poll it short-circuits — so [[src/components/widget/LimitsSection.tsx#LimitsSection]] renders a single muted "Showing cached data" sync-pill variant (slate, not red) rather than presenting the cached rows as live (the offline pill wins if both are present). A 401 from the usage API is treated as a stale access token, not a logout: [[src-tauri/src/fetcher.rs#fetch_claude_usage]] returns a `Paused` kind with a neutral message, the poll pushes `ProviderErrorKind::Paused` via [[src-tauri/src/lib.rs#push_paused_error]], and the LIMITS-header sync control shows its muted "Paused" variant with any cached rows still rendered (and the control alone on a first-run empty view) — never a login prompt or red error. To keep both guarantees, [[src-tauri/src/lib.rs#build_usage_data]] excludes `Paused` and `Stale` when picking the top-level `error`, so a stale-token or first-run-429 poll with no cached rows yet never surfaces a red "Failed to load usage data" — the muted badge/pill shows over an empty state instead. The red "Run: claude /login" guidance is reserved for a confirmed logout: no local credentials AND `claude auth status` reporting `loggedIn: false` (see 8d).
8b. Transport failures (DNS, connect refused, pre-response timeout) on Claude or MiniMax persist a per-provider network cooldown computed by [[src-tauri/src/lib.rs#compute_network_backoff]] — half-jitter exponential with a 60-second base, 30-minute cap, doubled per consecutive failure. The cooldown lives in the backend; the frontend `setInterval` poller keeps firing every 3 minutes but each call is short-circuited inside `refresh_usage_cache` and returns cached rows without a live HTTP request. The backend `tokio` loop hits the same short-circuit. No live request is made for either polling path during the cooldown. The poll pushes a typed `ProviderErrorKind::Network` so the LIMITS-header sync control can render a single consolidated "Offline — showing cached data" state instead of one red banner per provider. On any successful fetch, both cooldown timestamps and the consecutive-failure counter clear. The fast offline signal itself comes from [[src-tauri/src/config.rs#http_client]]'s 5-second connect timeout (15-second overall), so reqwest never hangs on a dead network.
8c. The kind classification originates in the fetcher: [[src-tauri/src/fetcher.rs#ClaudeUsageError]] exposes a Claude `kind` (`Credentials`/`Paused`/`RateLimited`/`Request`/`Api`/`Parse`) and [[src-tauri/src/fetcher.rs#MiniMaxUsageError]] exposes its own (`Unauthorized`/`RateLimited`/`Request`/`Api`/`Parse`). The polling layer in [[src-tauri/src/lib.rs]] maps Claude `Request` to `ProviderErrorKind::Network` (driving the network cooldown), `RateLimited` to a rate-limit cooldown plus a muted `ProviderErrorKind::Stale` (pushed on the fresh 429 and on every subsequent poll the cooldown short-circuits via the `UseCachedAsStale` decision from [[src-tauri/src/lib.rs#check_provider_cooldown]]), `Paused` (401, stale token) to the muted `ProviderErrorKind::Paused`, `Credentials` (no local token) to `Config` only after the logout confirmation in 8d (otherwise `Paused`), and the remaining variants to `Server`. MiniMax still maps `Unauthorized` to `Auth`. The mapping itself lives in the pure helpers [[src-tauri/src/lib.rs#classify_claude_error_kind]] and [[src-tauri/src/lib.rs#classify_minimax_error_kind]] so the match can be unit-tested without touching storage. Cooldown bookkeeping (skip-on-active, write-on-error, clear-on-success) goes through the per-provider [[src-tauri/src/lib.rs#ProviderCooldownKeys]] struct: each provider declares a constant value of that struct (`CLAUDE_COOLDOWN_KEYS`, `MINIMAX_COOLDOWN_KEYS`) wiring its four setting keys to the shared helpers [[src-tauri/src/lib.rs#check_provider_cooldown]], [[src-tauri/src/lib.rs#clear_provider_cooldowns]], [[src-tauri/src/lib.rs#write_rate_limit_cooldown]], and [[src-tauri/src/lib.rs#record_network_failure]]. Adding a new provider is a typed `<Provider>UsageError` in `fetcher.rs`, a fifth setting-key quartet, a constant `<PROVIDER>_COOLDOWN_KEYS` value, and a `classify_<provider>_error_kind` mapping — no further branching needed.
8d. When a Claude poll yields the `Credentials` kind (no local access token), the poller confirms the logout before warning. [[src-tauri/src/lib.rs#resolve_claude_logout_or_paused]] calls [[src-tauri/src/config.rs#claude_logged_in]], which spawns `claude auth status --json` UNCONFINED — a plain `tokio::process::Command` with the inherited environment and a ~15s timeout, NO Landlock/bwrap/sandbox-exec, NO prompt, NO `-p`, NO inference, and NO write to the credential store. Only `loggedIn: false` produces the red `Config` (logged-out) error; `loggedIn: true` or any inconclusive failure (binary missing, spawn error, timeout, parse failure) downgrades to `Paused` with cached rows and no warning. The verdict is cached for ~120s (`CLAUDE_AUTH_STATUS_CHECKED_AT_KEY` timestamp plus a `CLAUDE_AUTH_STATUS_LOGGED_IN_KEY` boolean) so the 3-minute poller spawns the CLI at most once per TTL; a successful live fetch clears the cache so a fresh login is recognized immediately.
8e. Configured CPA polling is the exclusive live usage path: Quill skips direct Claude, Codex, and MiniMax usage requests until CPA is disconnected. It reads `/auth-files`, preserves every runtime-only and file-backed account's local health metadata, then uses [[src-tauri/src/cpa/poll.rs#poll_account_snapshots]] for smoke-gated Claude/Codex window calls. A window failure removes only that account's numeric buckets. The source uses its own cooldown keys with the shared backoff helpers, persists `usage.cpa.last_accounts`, and restores those accounts with the latest CPA snapshot buckets during cached fallback.
Configured window polling is an explicit opt-in to Quill-induced off-device transmission: Quill asks loopback CPA to call Anthropic or OpenAI quota endpoints with the selected account token. The management key and upstream token remain local to Rust and CPA, but each scheduled window call is still an external request attributable to Quill's poll cadence.
8f. Unconfigured CPA has null impact: no client is constructed, no HTTP request or `cpa_phase_ms` log occurs, and the additive CPA arrays stay empty. A configured source adds a one-way URL fingerprint to the in-memory cache key; the management key never enters that key or any diagnostic. Pool aggregates are recomputed through [[features#Live Usage View#CPA Pool Aggregation]] as per-window means and never stored. Codex bucket identities normalize by duration rather than upstream primary/secondary field position. [[src/App.tsx#App]] combines enabled native status with the masked `get_cpa_connection_status` configured flag, so a completed no-source response cannot mark the titlebar live and an empty configured CPA inventory remains an active source.
9. The widget keeps direct buckets isolated from CPA buckets by source, then renders one authoritative row per provider. A CPA pool replaces its matching direct row while present; without a pool the direct row remains, and CPA-only state still has a row. Hidden direct errors do not degrade the titlebar while the CPA pool is authoritative. Account disclosure stays accessible, health states remain distinct, and missing account windows stay nonnumeric. Claude seeds canonical windows; Codex derives its shared schema only from returned durations, so absent limits leave no synthetic cells or resets. Unsupported providers collapse into a neutral other-account line; no view selects a single bucket for history any more
10. `emit_usage_updates()` emits the complete safe `UsageData` snapshot as `usage-updated`, rebuilds the backend-owned indicator state, and emits `indicator-updated`. App applies the snapshot directly, refreshes masked CPA connection status, and guards asynchronous listener cleanup for React Strict Mode.

CPA connection setup is a separate guarded mutation before its poll path becomes eligible. [[src-tauri/src/lib.rs#set_cpa_connection]] validates the loopback management endpoint and auth-file shape, performs one provider smoke call, then persists the connection and `usage.cpa.window_smoke.{claude,codex}` verdicts. The management key remains Rust-owned after submission and is omitted from every response.

Disconnect reverses the source completely: [[src-tauri/src/integrations/manager.rs#clear_cpa_connection]] deletes connection/runtime settings plus CPA snapshots and hourly aggregates while holding `integration_mutation_guard`; the command then bumps the usage-cache epoch before rebuilding the next emitted snapshot.

## Indicator Preference Pipeline

The status indicator has one backend-owned provider preference shared across the tray and the [[features#Settings Window]]'s Integrations tab.

1. `useIntegrations()` loads `get_indicator_primary_provider` alongside provider statuses so the Integrations tab starts from the persisted preference
2. The Integrations tab renders an `Auto` option plus enabled providers, and preserves a disabled unavailable option when a saved provider is temporarily missing
3. Changing the selector invokes `set_indicator_primary_provider`, which stores the configured provider in the settings table
4. The backend recomputes `StatusIndicatorState`, emits `indicator-updated`, and updates the tray summary from that backend-owned payload. A matching CPA pool is authoritative over account-qualified buckets and available independently of native provider status; its Claude and Codex pool keys map to the normal short and weekly windows. Direct native buckets remain the fallback when no matching pool exists.
5. `useIntegrations()` listens for `indicator-updated` to keep all mounted selector instances synchronized
