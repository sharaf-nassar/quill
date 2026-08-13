# Plan: live-session-tracker

Implementation order for `spec.md`. Each task maps 1:1 to a bead; file:line
anchors reference the tree at design time (commit 823ba82 + in-flight
watcher).

## Task graph

```
T1 extract primitives
 └─ T2 tracker core ── T3 claude fold ─┐
                    └─ T4 codex fold  ─┼─ T5 watcher wiring
                                       └─ T6 read-path rewire ── T7 delete legacy ── T10 lat.md sync
T8 frontend events (after T6)                                     T11 verification (last)
T9 backfill-gate bug (independent, anytime)
```

## Tasks

**T1 — Extract evidence primitives from the scanner.** Pure-refactor step
while the old scanner still runs. Extract the four Claude rules inlined in
`transcript_scan.rs::session()` (activity/attachment filter 383-396,
first-record started_at/cwd 391-397, journal result filter 410-423, agent
open precedence 428-451) into named functions beside the already-pure
primitives (`tool_result_ids`, `read_agent_meta`, `read_appended`, Codex
head/tail/model/root/activity/turn helpers). Do not touch shared deps:
`transcript_identity::codex_metadata`, `sessions::codex_text_blocks`,
`sessions::has_nonempty_codex_assistant_output`,
`model_usage::validate_model_id`, the four `sessions.rs` walker/id helpers.

**T2 — LiveTracker core.** New `live_tracker.rs`: state structs, per-file
`FileTail` offsets, `apply_paths` fold engine on `read_appended` semantics,
`sweep()` (walkers + stat diff + eviction on `IDLE_AFTER`), toggle methods,
debounced `sessions-live-updated` emission (constant in lib.rs). Tail
mechanics tests: partial line, truncation reset, appended-bytes-only.

**T3 — Claude fold rules.** Root/agent/journal folding per spec; lazy
`.meta.json`; per-session resolved-tool-use-id set; model from agent
assistant records. Port tests: nested depth-2 spawn, workflow journal,
appended-bytes, abandoned spawn + idle eviction, bookkeeping-no-reopen.

**T4 — Codex fold rules.** Incremental `thread_id → path` index; cold-start
head+tail parse; boundary flips; substantive-activity rule; first-model
rule. Port the six Codex rule tests; fix the vacuous
`quiet_codex_rollouts_leave_the_scan` assertion (asserts the Claude map on a
Codex fixture, transcript_scan.rs:1573) in the ported version.

**T5 — Watcher + startup wiring.** Feed `tracker.apply_paths` from
`transcript_watcher::admit_pending` (single admission site, line 322);
`sweep()` on startup, on overflow recovery, and on the 120 s retry tick.
Manage `Arc<LiveTracker>` in `setup` (lib.rs:5820-5826 region), honoring
activity-tracking/provider settings at construction.

**T6 — Read-path rewire.** In `get_session_breakdown` (lib.rs:3547-3592):
drop `refresh_from_transcripts`, swap `session_ranking_keys`/`merge` to the
tracker, drop the model-evidence callback and rescan nudge; terminal and
runtime passes unchanged. Rewire settings commands (lib.rs:3711, 3761,
3782) to tracker toggles. Rewire cross-file tests:
`session_breakdown_command_overlay_preserves_nullable_ipc` (lib.rs:6389)
and `observed_activity_reopens_limited_stored_session_with_metrics`
(storage.rs:20425) against a test-only tracker seeding method.

**T7 — Delete legacy.** `transcript_scan.rs` orchestration + file,
server.rs 64-493 live-state types (keep sanitizers by moving them, keep
`is_supported_observed_hook_event`), reconciler test module 2205-2832 (keep
`hook_validation_accepts_provider_terminal_events`),
`get_observed_agent_model_evidence` + its test,
`claude_row_awaits_child_model` + nudge. `cargo test` green.

**T8 — Frontend event subscriptions.** `sessions-live-updated` in
`useCachedInvokeEvents.ts:6` INGEST_EVENTS and `useBreakdownData.ts:50`
sessions list; extend the frontend cache test per
`lat.md/frontend-cache-tests` conventions.

**T9 — Lifetime-totals gate bug (independent).** Root-cause why
`rollup_meta.runtime_backfill_status` is not `'complete'` on the primary
host (gate at storage.rs:13391-13405 nulls `active_runtime_secs`,
`agent_count`, `agent_runtime_secs`). Fix if small; otherwise file the fix
as its own bead with findings.

**T10 — lat.md sync.** Rewrite `data-flow.md` "Transcript-Derived Session
Snapshots" → "Live Session Tracker"; update Hook Telemetry Pipeline
cross-refs, `backend.md` breakdown flow, `frontend.md` event list; replace
`live-subagent-count-tests.md` sections to match ported/new tests with
`@lat:` refs; `lat check` green.

**T11 — End-to-end verification.** Isolated fixtures only (never attach to
or mutate the live Quill window — bd memory). Both providers: agent rail
appears on spawn, closes on result/boundary, survives Quill restart via
sweep; read path holds the 300 ms budget with scan-on-read gone; remote-row
fields stay null.

## Risks

- `.meta.json` races the agent transcript — mitigated by lazy retry; test.
- notify semantics differ per platform (macOS fsevent coarseness) — the
  120 s sweep is the invariant safety net; keep it unconditional.
- Codex parent chains can reference rollouts outside the live window — the
  incremental index covers the full corpus (5.5 k entries, built once at
  startup sweep).
