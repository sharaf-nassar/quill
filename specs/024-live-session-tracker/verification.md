# Verification: live-session-tracker (T11)

End-to-end verification of the live tracker against isolated fixture
transcripts. No live Quill window was attached to, resized, or otherwise
touched; every scenario runs over `tempfile` provider roots inside
`cargo test`.

Tree: `wt/orch-quill-zpc8.11` on top of `a2999b5` (T1-T10 landed).

## Scenarios

All scenarios exercise the real read surface the `get_session_breakdown`
command calls — `LiveTracker::session_ranking_keys()` followed by
`LiveTracker::overlay()` — over state folded from fixture transcripts on disk.

| Scenario | Where | Result |
|---|---|---|
| Claude spawn appears in the agent rail with the model from its own transcript | `live_tracker.rs::a_claude_spawn_reaches_the_read_path_and_survives_a_restart` | pass |
| Claude rail closes on the spawning call's `tool_result`, session stays live | same | pass |
| Claude rail survives a Quill restart (fresh tracker, startup sweep only) | same | pass |
| Remote-host row keeps null `observed_agents` and its retained metrics | same (+ Codex twin, + `session_breakdown_command_overlay_preserves_nullable_ipc`) | pass |
| Codex sub-agent rollout opens on `task_started` with its own model | `live_tracker.rs::a_codex_spawn_reaches_the_read_path_and_survives_a_restart` | pass |
| Codex rail closes on `task_complete` | same | pass |
| Codex rail survives a restart via sweep | same | pass |
| Depth-2 Claude spawn closes from the parent agent transcript | `a_nested_spawn_resolves_from_the_parent_agent_transcript` | pass |
| Workflow agent follows its journal | `workflow_agents_resolve_from_their_journal` | pass |
| Missed watcher events recovered by the periodic sweep | `a_sweep_skips_transcripts_untouched_past_the_cutoff`, `quiet_codex_rollouts_leave_the_fold` | pass |
| Observed-only synthesis for a session storage has no row for | `overlay_synthesizes_live_rows_and_reranks_them`, both e2e tests | pass |

## Timings

`live_tracker.rs::the_read_path_costs_a_map_lock_rather_than_a_scan`, debug
build (`cargo test`, unoptimized — the release path is faster), corpus of 400
folded sessions (200 Claude trees with an open agent each + 200 Codex
rollouts), read sampled 20x over 200 storage rows:

| Run | cold sweep (full corpus fold) | warm sweep (120 s tick) | read p95 | read max |
|---|---|---|---|---|
| 1 | 52.1 ms | 18.6 ms | 13.3 ms | 13.3 ms |
| 2 | 49.0 ms | 20.7 ms | 14.6 ms | 14.6 ms |
| 3 | 45.5 ms | 17.5 ms | 14.5 ms | 14.5 ms |

- **Read path budget (300 ms): held**, asserted in the test. p95 ≈ 14 ms, ~20x
  headroom, and the read opens no transcript at all — the cost is the map lock
  plus row re-ranking. Scan-on-read is gone.
- **Sweep cost**: cold fold of the whole corpus lands in the 45-52 ms band, the
  steady-state tick (stat-only, nothing appended) in 17-21 ms — inside the
  50-80 ms design band, paid once per 120 s rather than once per read.
- Structural updates: `apply_paths` folds appended bytes synchronously and
  `notify()` coalesces into one `sessions-live-updated` per 250 ms window, so a
  spawn or closure reaches the frontend inside the debounce window.

## Original regression

Empty agent rail on live sessions is resolved for the tracker-owned fields: a
freshly spawned agent is present in `observed_agents` on the row the command
returns, in both providers, before and after a restart. Lifetime agent
*totals* remain DB-owned and depend on T9 (`runtime_backfill_status` gate),
which landed separately.

## Gates

- `cargo test` (src-tauri): 379 passed, 0 failed, 4 ignored.
- `lat check`: all checks passed.
