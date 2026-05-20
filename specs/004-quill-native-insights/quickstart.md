# Quickstart: Verifying the Quill-Native Session Insights Stream

Maintainer walkthrough. Each verification maps 1:1 to a Success Criterion. Run from a build of branch `004-quill-native-insights`. The app DB is `~/.local/share/com.quilltoolkit.app/usage.db`; run records live in `learning_runs` (inspect via Python `sqlite3` stdlib, read-only).

## Pre-req

Populated local session index (open Session Search once if needed). At least one project with recent Claude (and ideally Codex) sessions.

## V1 — Local-only, no external command (SC-001, SC-002 · US1)

1. Make the external command unreachable for the app, e.g. start the app with a `PATH` whose `claude` resolves but rename/hide so a bare `claude /insights` would fail — OR simply trace process spawns.
2. Run **Analyze** in the Learning section (full mode, default scope).
3. **Expect**: run completes; `learning-log` shows `Stream C: …` lines (selection count, extracted N patterns); **no** `claude /insights` process is spawned and `~/.claude/usage-data/facets` is **not** read during the run (verify via `strace -f -e trace=execve` / `lsof`, or absence of any `/insights` log line).
4. **Pass**: run status `completed`, rules produced, zero external-command involvement.

## V2 — Metadata persisted for stream_c (SC-003 · US2)

1. After V1, query the latest run: `SELECT inference_metadata FROM learning_runs ORDER BY id DESC LIMIT 1;`
2. **Expect**: the JSON array contains an entry with `"phase":"stream_c"` including token counts, model id, durations, cost, stop reason.
3. Repeat with a forced failure (V5) and confirm a `stream_c` entry with `"success":false` and a `failure_kind`.
4. **Pass**: `stream_c` metadata present for both a completed and a failed run (previously absent for all runs).

## V3 — Text-log fidelity & concurrency preserved (FR-010 · SC-007)

1. Watch the live `learning-log` during a run.
2. **Expect**: the three streams start together ("Full mode: launching Stream A, Stream B, Stream C in parallel"); Stream C lines follow the established shape (`Stream C: extracted {n} patterns, {k} verdicts`); synthesis runs only after all three; total wall-clock ≈ prior runs (no serial inflation).
3. **Pass**: parallel dispatch intact, log shapes match A/B, latency in the same range as a pre-feature run of similar scope.

## V4 — Insights-only run produces rules (SC-004 · US3)

1. Pick a scope/project where Stream A (observations) and Stream B (git) yield nothing (e.g. a project with no unanalyzed observations and an empty/absent git history) but with real session history.
2. Run **Analyze**.
3. **Expect**: run completes and creates ≥1 rule attributable to session insights (previously this failed with `"No streams produced findings"`).
4. **Pass**: `status=completed`, `rules_created ≥ 1`.
5. **Negative control**: a scope with all three streams empty still fails with `"No streams produced findings"` and `rules_created=0` (unchanged).

## V5 — Specific failure cause (SC-005 · US2)

1. Force a Stream C inference failure (e.g. run while signed out, or with the model unavailable / rate-limited).
2. Run **Analyze**; inspect `learning_runs.logs` and `inference_metadata` for the run.
3. **Expect**: a `Stream C: …: <specific cause>` log line (NotSignedIn / RateLimited / SchemaValidationFailed / TimedOut / …) and a `stream_c` metadata entry with the matching `failure_kind` — not only the aggregate "no findings" message.
4. **Pass**: specific cause is recoverable from the run record 100% of the time.

## V6 — Provider scoping (FR-007 · US2)

1. Run **Analyze** scoped Claude-only, then Codex-only, then combined.
2. **Expect**: the Stream C selection (visible via log count / digested session ids) draws only from in-scope provider sessions; Codex-only no longer hits the old "Skipping Claude insights for Codex-only analysis" short-circuit — it analyzes Codex sessions.
3. **Pass**: in-scope-only sessions feed Stream C for each scope.

## V7 — Determinism & no-regression baseline (SC-006)

1. With a frozen local index, run **Analyze** twice with identical scope; confirm Stream C selects the same session set (deterministic `get_session_breakdown` ordering).
2. Compare discovered-rule count/usefulness against a pre-feature run on the same data (the prior approach's *effective* signal = aggregated friction/outcome/summary).
3. **Pass**: deterministic selection; rule count & reviewer-judged usefulness ≥ the prior baseline (parity is the floor; FR-002a makes the signal a superset).

## V8 — Secret redaction & top-level-only selection (SC-008 · FR-012/FR-013)

1. Seed a recent in-scope session whose transcript contains obvious secrets (e.g. `OPENAI_API_KEY=sk-…`, a bearer token, a `.env` line) and ensure at least one sub-agent/sidechain session exists in the window.
2. Run **Analyze**; capture the exact content submitted for Stream C (debug log of the assembled prompt, or instrument the digest step).
3. **Expect**: every seeded secret pattern appears masked (e.g. `‹redacted›`) in the submitted content while the surrounding behavioral text is intact; the selected session set contains **no** sidechain/sub-agent session ids and the recency cap is filled only by top-level sessions.
4. **Pass**: 100% of detected secret patterns masked with behavioral signal preserved (SC-008); zero sidechain sessions selected (FR-013).

## Cleanup / regression sweep

- `grep -rn "/insights\|InsightsData\|InsightsFacet\|gather_insights" src-tauri/src/` → no learning-pipeline matches (FR-011).
- `cargo test --lib` green (existing `cc_client` tests unaffected).
- `lat check` passes (lat.md synced: data-flow Stream C step, features Learning System, backend StreamC note).
