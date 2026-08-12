# Verification: live-subagent-count

## Verdict

Pass under human Option B: functional, UI, latency, documentation, and local release gates pass; the ten-sample frozen-corpus p95 is explicitly waived and no p95 measurement is claimed.

## Targeted Regression Gates

All owning-layer tests passed in attempt 3.

| Gate | Result |
|---|---|
| `node --test scripts/session-subagents.test.mjs` | Pass: 2 tests |
| `node --test src-tauri/claude-integration/scripts/hook-runtime.test.cjs` | Pass: 7 tests |
| `node --test src-tauri/codex-integration/scripts/hook-observe.test.cjs` | Pass: 3 tests |
| `cargo test lifecycle_truth_table_covers_both_providers` | Pass |
| `cargo test session_breakdown_command_overlay_preserves_nullable_ipc` | Pass |
| `cargo test get_session_breakdown_rolls_up_subagent_tokens` | Pass |
| `cargo test hook_observation_preserves_same_time_agent_identity` | Pass |
| `cargo test lifecycle_observers_follow_activity_tracking` | Pass |
| `cargo test hook_observer_contract_preserves_registrations_and_activity_gate` | Pass |

## Full Quality Gates

Repository Rust, frontend, release, formatting, and static-analysis gates passed.

| Gate | Result |
|---|---|
| `cargo fmt --check` | Pass |
| `cargo clippy --all-targets -- -D warnings` | Pass; no compiler warnings |
| `cargo test` | Pass |
| `npm run lint` | Pass |
| `npm run typecheck` | Pass |
| `npm run build` | Pass |
| `npm run knip` | Pass |
| `node --test scripts/cached-invoke.test.mjs` | Pass: 9 tests |
| `npm run tauri -- build --ci --no-sign` | Pass: optimized binary and Linux AppImage/updater bundle |
| `git diff --check` | Pass |
| `pre-commit run --all-files` | Pass |
| `lat check` | Pass |

The default signed Tauri build generated the optimized binary and AppImage, then stopped after bundling because this worktree has no `TAURI_SIGNING_PRIVATE_KEY`. That invocation is not counted as passed. The verified local release gate used the documented `--no-sign` mode; CI signing was not performed locally.

Tauri printed existing bundle-identifier and legacy-updater advisories plus the expected unsigned-build warning. Clippy, ESLint, TypeScript, Vite, tests, and source formatting introduced no new warnings.

## Isolated UI Evidence

Vite's dev-only browser IPC fixtures ran in headless Chrome with a temporary profile and no Tauri process, live Quill window, or production data.

At 451 px, every 435 px Sessions row had `scrollWidth === clientWidth` and zero right overflow. The long project used 230 px of a 237 px intrinsic width with `overflow: hidden` and `text-overflow: ellipsis`.

At 320 px, every 304 px row again had no row or document overflow. The long project shrank to 99 px against a 237 px intrinsic width and ellipsized, leaving count, provider, tokens, and recency intact.

Both viewports preserved the DOM order `project -> optional count -> provider -> tokens -> recency`. Only positive fixtures rendered (`+3`, `+1`); zero and null created no metadata element. Each positive count exposed one label only: `3 subagents observed open` or `1 subagent observed open`. The idle `+1` row remained visible independently of its recency dot.

The Sessions retention note kept its tool-activity disclosure and now has title `Tool activity recorded before this date was pruned.` Neither viewport contained `Sub-agent trees` or `marked, not zeroed`.

## Refresh Latency

An isolated mounted Sessions view was reloaded, its mocked result changed from `+3` to `+9`, and `hooks-observed-updated` was emitted immediately after the initial request.

- Accepted event to refreshed `get_session_breakdown` start: **4,955.1 ms** (required at most 5,000 ms).
- Accepted event to mounted `+9` row: **4,960.0 ms** (required at most 6,000 ms).

No feature-specific timer, poller, or transport was used.

## Stale-Projection Audit

The obsolete Sessions retention claims are absent from runtime and architecture documentation.

```text
rg -n -i "Sub-agent trees|marked, not zeroed|get_session_breakdown.*pruned tool|mixed[- ]horizon|dagger(ed)?" src lat.md scripts src-tauri --glob '!**/target/**'
```

Result: no matches. The new test spec and storage assertion intentionally retain only the negative contract that Sessions SQL excludes historical agent state.

## Frozen Corpus And Waived p95

The pinned read-only corpus remains authenticated, but no latency number is inferred from it.

- Path: `/home/mamba/.local/share/com.quilltoolkit.app/benchmark-corpora/widget-query-perf/usage-2026-08-02.db`
- Size: 13,525,123,072 bytes.
- Mode: `0444`.
- SHA-256: `c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e`.
- Attempt 1 authenticated schema version 37.

The exact archived harness imports removed trace APIs and calls removed `Storage::init_widget_query_benchmark` and `Storage::explain_session_breakdown_query`, so it no longer compiles against current Storage APIs. Human Option B on 2026-08-05 waives only the required ten-sample p95 evidence.

The harness was not restored, adapted, replaced, or rerun in attempts 2 or 3. The session-breakdown p95 check is **waived, not passed**, and this verification makes no p95 claim.

## Rollback

Rollback requires reinstalling the previous release and restarting Quill; no data migration or data rollback is required.

The feature uses bounded process-local state and the existing audit table. A diff audit from the pre-feature specification commit found no `user_version`, `CREATE TABLE`, `ALTER TABLE`, or migration change in `storage.rs`.

## Documentation Traceability

Architecture and behavior updates cover backend, hook data flow, Sessions UI, retention cleanup, and the nullable observed-count contract.

`lat.md/live-subagent-count-tests.md` defines nine authorized leaf specs. Each has exactly one adjacent `@lat:` reference at its owning test group, and `lat check` passes.
