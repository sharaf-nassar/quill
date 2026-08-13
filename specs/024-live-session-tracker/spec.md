# Spec: live-session-tracker

## Problem Statement

Sessions rows derive their live facts (open agents, liveness, activity) from
three parallel pipelines — retained DB ingest, a stateless transcript
re-scanner (`transcript_scan.rs`), and hook observations — fused at read time
by `SnapshotReconciler`/`ObservedSubagentState` with precedence rules,
staleness cutoffs, last-write-wins snapshots, and a retained-model-outranks-
transcript overlay. The fusion layer is the dominant complexity (~30
protected test behaviors exist only because two sources can disagree), the
scan runs on the read path behind a throttle, and the feature has regressed:
open agents and agent totals no longer render.

## Approved Decisions

- **1A** — lifetime totals (`active_runtime_secs`, `agent_count`,
  `agent_runtime_secs`) stay DB-sourced from retained analytics.
- **2A** — remote hosts are a real use case (server binds 0.0.0.0). Agent
  detail stays local-only; remote rows keep hook-derived liveness and
  honestly-null agent fields.
- **3A** — the Codex 8-event hook observer stays as-is for the Hooks view.

### Amendments discovered during investigation

- `current_turn_runtime_secs` is already retained-table-derived
  (`runtime_turn_state` + `session_events` open tail, storage.rs:13455-13612)
  and deliberately not gated on backfill coverage. It stays DB-owned; the
  tracker does not duplicate it.
- The hook-handler "poke the tracker" idea is dropped (YAGNI): terminal
  evidence already reaches rows via `hook_invocations` →
  `populate_session_terminal_evidence`, and the frontend hides the agent rail
  on non-live rows, so a poke would change nothing visible.
- `SessionBreakdown` / `ObservedSessionAgent` wire contracts are unchanged.
  `runtime_as_of_ms` / `active_runtime_rate` are retained-derived and
  survive. Frontend changes are limited to event subscriptions.

## Principle: one fact, one owner

| Sessions-row field | Owner |
|---|---|
| tokens, turn_count, project, first_seen | DB (`token_snapshots`, `response_times`) — unchanged |
| lifetime runtime, agent count/runtime, current-turn runtime, rate, as-of | DB (`runtime_hourly`, `runtime_turn_state`, `session_events`) — unchanged |
| `ended_at` | DB (`hook_invocations` projection) — unchanged |
| liveness bump, observed-only synthesis, open agents (id, type, model, open) | **LiveTracker** (new) — sole owner, local host only |
| per-open-agent `runtime_secs`/`runtime_active` | DB overlay keyed by tracker-supplied agent ids — unchanged seam |

No column has two owners, so the reconciler, snapshot generations, and the
retained-model-outranks-transcript rule are deleted rather than reworked.

## Design

### LiveTracker (`src-tauri/src/live_tracker.rs`, new)

An in-memory incremental fold over local transcript files, fed by the
existing `transcript_watcher` fs events. Replaces `TranscriptScanner` +
`SnapshotReconciler` + `ObservedSubagentState`.

State (single mutex): `sessions: HashMap<SessionKey, LiveSession>` plus
`files: HashMap<PathBuf, FileTail>` (byte offset, role) and an incremental
Codex `thread_id → path` index. `LiveSession` holds cwd, started_at,
last_activity, resolved spawn-tool-use ids, and
`agents: HashMap<agent_id, LiveAgent {agent_type, model, open}>`.

Operations:

- `apply_paths(batch)` — fold complete appended lines from each file's
  stored offset (`read_appended` semantics: partial trailing line left for
  the next pass; shrink resets to a cold-start read).
- `sweep()` — walk both provider roots with the existing `sessions.rs`
  discovery walkers, stat files, fold any size-vs-offset diff, evict
  sessions idle past `IDLE_AFTER` (15 min, unchanged). Runs at startup
  (cold start), on watcher overflow/recovery, and on the watcher's existing
  120 s retry tick as the missed-event backstop.
- `session_ranking_keys()` / `overlay(rows, …)` — the read-path surface,
  mirroring today's call shape minus the model-resolution callback and
  snapshot bookkeeping. `overlay` bumps `last_active`, sets
  `observed_agents`, synthesizes observed-only rows for tracker sessions
  with a validated root cwd absent from SQL, then re-sorts and truncates to
  the limit (clamp stays in lockstep with storage.rs:13260).
- `set_activity_tracking_enabled` / `set_provider_enabled` — same toggle
  semantics as today (clear state, next sweep rebuilds).
- Emits debounced `sessions-live-updated` after any state-changing fold.

Trust boundary: transcript-derived strings keep today's sanitizers
(agent-type length cap, absolute-cwd check — server.rs:105-118 move here).

### Fold rules (evidence rules unchanged, applied incrementally)

*Claude* — root file: first record supplies started_at/cwd; timestamped
non-`attachment` records advance activity. Agent files
(`subagents/**/agent-*.jsonl`): sibling `.meta.json` supplies
`toolUseId`/`agentType` (read lazily, retried on later events until
present); agent is open until its `toolUseId` appears in a `tool_result`
anywhere in the tailed session tree (per-session resolved-id set — covers
the depth-2 case natively). Workflow agents (`subagents/workflows/wf_*/`)
open on journal `started`, close on journal `result`. **Model comes from
the agent transcript's own assistant records** (`message.model`, verified
present) — this deletes `get_observed_agent_model_evidence` and the
model-rescan nudge. Unresolved spawn silent past `IDLE_AFTER` = abandoned.

*Codex* — first sight of a rollout: bounded head parse
(`read_codex_head`/`codex_metadata`) for identity + parent chain
(`codex_root` walk against the incremental thread index), first
`turn_context` model (`read_codex_model`), and bounded tail
(`read_codex_tail`/`codex_agent_running`) for current turn state — the
cold-start primitives are kept verbatim. Thereafter appended
`task_started`/`task_complete`/`turn_aborted` boundaries flip the open bit
and substantive records (`codex_activity_timestamp` rule) advance root
activity; post-Stop bookkeeping cannot.

### Read path (`get_session_breakdown`, lib.rs:3547-3592)

Stage 1 (`refresh_from_transcripts` scan-on-read) is deleted — reads cost a
map lock. Stages: tracker keys → SQL (observed-keys CTE unchanged) →
`tracker.overlay(rows)` → terminal projection (unchanged) → runtime pass
(unchanged; its two live-state seams already key on the tracker map shape).
The Claude model-rescan nudge (lib.rs:3587-3589) is deleted.

### Deletions

`transcript_scan.rs` orchestration (scanner struct, staging types, throttle,
per-pass memos; primitives move to the tracker), server.rs live-state types
and `ObservedSubagentState` (SessionKey/snapshot/reconciler/merge,
lines 64-493 except sanitizers and `is_supported_observed_hook_event`),
`get_observed_agent_model_evidence` (storage.rs:13656-13708),
`claude_row_awaits_child_model` + nudge (lib.rs:880-894), reconciler test
module (server.rs:2205-2832 except the hook-validation test), and the
scan-throttle test ceremony.

### Frontend

Two event-list additions (`sessions-live-updated` in
`useCachedInvokeEvents.ts` INGEST_EVENTS and `useBreakdownData.ts` sessions
invalidation list). No contract, formatting, or row-grammar changes.

## Reliability

| Failure | Handling |
|---|---|
| missed fs events | watcher overflow → recovery sweep; unconditional sweep on 120 s retry tick |
| truncation/rewrite | size < offset → cold-start refold of that file |
| record mid-write | partial line buffered until its newline lands |
| agent crash, no closure | unresolved spawn idle past 15 min = abandoned (unchanged) |
| meta.json not yet written | agent open-typeless; lazy retry per event |
| Quill restart | clean rebuild via startup sweep; no persisted live state |
| remote host | no local transcripts; row keeps DB metrics + hook liveness, agent fields null |

## Known separate defect

Lifetime totals render `—` whenever `rollup_meta.runtime_backfill_status ≠
'complete'` (storage.rs:13391-13405 gate). That is retained-pipeline
behavior this redesign deliberately keeps; the stuck gate is a distinct bug
tracked as its own bead.

## Non-Goals

Persisting live state; agent detail for remote hosts; changing retained
ingest, token pipeline, hook audit, or the Hooks view; wire-contract or
visual changes; touching the in-flight event-driven retained-ingest watcher
beyond adding the tracker feed.
