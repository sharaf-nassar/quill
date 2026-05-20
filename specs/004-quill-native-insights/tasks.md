---
description: "Task list for Quill-Native Session Insights Stream"
---

# Tasks: Quill-Native Session Insights Stream

**Input**: Design documents from `/specs/004-quill-native-insights/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/stream-c.md, quickstart.md (all present, reconciled with Clarifications Session 2026-05-16)

**Tests**: No automated-test tasks generated. Project/user policy authors test code only on explicit request (research R-9); spec acceptance is verified via `quickstart.md` V1–V8. Add test tasks later only if the user requests TDD.

**Organization**: Tasks grouped by user story (US1 P1, US2 P2, US3 P3) for independent implementation and verification. Backend-only; all paths under `src-tauri/src/`.

## Format: `[ID] [P?] [Story?] Description with file path`

- **[P]**: Parallelizable (different file, no incomplete dependency)
- **[Story]**: US1/US2/US3 (user-story phases only; Setup/Foundational/Polish carry no story label)

## Path Conventions

Single Rust crate at `src-tauri/`. Behavioral edits land in `src-tauri/src/learning.rs`; `cc_client.rs` gets a comment refresh; `sessions.rs`/`storage.rs`/`prompt_utils.rs` are consumed read-only; `lat.md/` synced in Polish.

---

## Phase 1: Setup

- [X] T001 Add Stream C tuning constants (`STREAM_C_LOOKBACK_DAYS = 14`, `STREAM_C_MAX_SESSIONS = 40`, `STREAM_C_CONTEXT_BUDGET = 48 KiB`) with doc comments after the import block in `src-tauri/src/learning.rs` (research R-3/R-4). ✅ compiles.

## Phase 2: Foundational (blocking prerequisites for all user stories)

- [X] T002 [P] Implemented `select_sessions_for_insights(storage, provider) -> Result<Vec<SessionBreakdown>, String>` in `src-tauri/src/learning.rs` wrapping `get_session_breakdown(STREAM_C_LOOKBACK_DAYS, None, provider, Some(STREAM_C_MAX_SESSIONS))` — cross-project, provider-scoped, recency-ordered. **Note:** `SessionBreakdown` already rolls sub-agent chains into the top-level `(provider, session_id)` row (models.rs:155-157), so FR-013 is satisfied at the data layer here; the residual FR-013 concern (excluding sub-agent transcript *content*) moves to T004's `fetch_content` seam. Error returned, not swallowed (FR-005 spirit). ✅ compiles.
- [X] T003 [P] Implement the secret/credential redaction helper in `src-tauri/src/prompt_utils.rs` (alongside `compress_observation`/`safe_truncate`): mask API keys, access/refresh/bearer tokens, `.env`-style `KEY=value`, and recognizable credentials with a fixed masking token, preserving surrounding behavioral/semantic text; rule-neutral and idempotent (FR-012; **design-bearing — pattern set + masking token are the meaningful choices, see research R-4**).
- [X] T004 Implement per-session digest assembly + context-budget allocation in `src-tauri/src/learning.rs`: for each selected top-level session pull local content via `sessions_index.get_context`/`search`, reduce to intent + outcome + tool/code/command/error signal using `prompt_utils::compress_observation`/`safe_truncate`, apply the T003 redaction pass, allocate `STREAM_C_CONTEXT_BUDGET` across sessions (per-session cap = budget/count, floor enforced, drop oldest beyond budget), skip too-thin sessions; produce the transient `SessionDigest` set (FR-008, FR-009, FR-012; depends on T002+T003; **design-bearing — the R-4 key decision/implementation-contribution point**).

## Phase 3: User Story 1 — Analysis no longer depends on the external command (Priority: P1) 🎯 MVP

**Goal**: A run derives Stream C signal entirely from Quill local data, produces `StreamFindings`, and the `/insights` subprocess + parsing are gone.

**Independent test**: quickstart V1 — external command absent from PATH, populated local index, run Analyze ⇒ run completes, Stream C contributes findings, no `/insights` spawned, no `~/.claude/usage-data` read.

- [X] T005 [US1] Implement `analyze_sessions_stream(storage, provider, existing_rules_summary, app, run_id) -> (Option<StreamFindings>, Vec<String>, Option<InferenceCallMetadata>)` in `src-tauri/src/learning.rs`: assemble digests (T004), build the Stream B-style prompt + preamble for the FR-002 rule-relevant dimensions + existing-rule verdicts (empty-output convention `{"patterns": [],"verdicts": []}`), call `cc_client::invoke_typed::<StreamFindings>(InvokeArgs { phase: Phase::StreamC, prompt, preamble, model: Model::Haiku, max_tokens: 4096 })`, map Ok→`(Some,logs,Some(meta))` with the `stream_log!("Stream C: extracted {} patterns, {} verdicts", …)` line (FR-002/FR-002a, FR-003, FR-010; contract steps 4–5).
  - **As-built note (feature 005, 2026-05-18)**: the implemented call uses `model: Model::Sonnet46` (pinned `claude-sonnet-4-6`), not `Model::Haiku` — the planned Haiku assignment was superseded for a single-model pipeline (feature 005 US5 T060/H-7, L-1). `max_tokens = 4096` and `Phase::StreamC` are unchanged; the original task text is preserved as the historical record.
- [X] T006 [US1] In the full-mode `tokio::join!` of `src-tauri/src/learning.rs`, replace the `gather_insights(...)` arm with `analyze_sessions_stream(...)`, change the destructure to the 3-tuple `(insights_result, insights_logs, insights_metadata)`, extend logs, and push `insights_metadata` into `inference_metadata_records` (FR-010; contract caller-side).
- [X] T007 [US1] Change `synthesize_findings` in `src-tauri/src/learning.rs`: parameter `insights: Option<&InsightsData>` → `insights_findings: Option<&StreamFindings>`; rebuild the in-prompt `insights_text` by iterating `patterns` (same formatting approach used for obs/git findings) instead of `InsightsData` fields; update the call site to pass the unpacked insights `StreamFindings` (FR-003).
- [X] T008 [US1] Delete `gather_insights`, `InsightsData`, `InsightsFacet`, and the local `insight_log!` macro from `src-tauri/src/learning.rs`; confirm `dirs::home_dir()` / `crate::config::shell_path()` have other callers before touching imports; ensure no `/insights` or facets-dir code path remains (FR-001, FR-011).
- [X] T009 [P] [US1] Refresh the `Phase::StreamC` doc comment in `src-tauri/src/cc_client.rs` — it is no longer "reserved for a future migration … runs `claude /insights --print` directly"; state it is the active Quill-native session-insights path. No logic change (research R-5).

**Checkpoint**: US1 independently shippable — local-only Stream C producing findings; external command fully removed.

## Phase 4: User Story 2 — Observable, scoped, fails loudly (Priority: P2)

**Goal**: `stream_c` inference metadata persisted (success + failure), provider scope honored incl. Codex, failures report a specific cause.

**Independent test**: quickstart V2 (stream_c metadata present), V5 (specific failure cause), V6 (provider scope), V8 (redaction + top-level-only).

- [X] T010 [US2] Verify/enforce provider scoping end-to-end in `src-tauri/src/learning.rs`: the T002 helper restricts sessions to the in-scope provider(s); explicitly confirm the old "Skipping Claude insights for Codex-only analysis" short-circuit is gone so Codex-only runs analyze Codex sessions (FR-007; quickstart V6).
- [X] T011 [US2] Ensure the `analyze_sessions_stream` error path emits `stream_log!("Stream C: …: {e}")` with the specific `InferenceError` cause and returns `failed_metadata(Phase::StreamC, 4096, &e)`, and that the join site pushes `insights_metadata` for both success and failure into the persisted `learning_runs.inference_metadata` (FR-005, FR-006; contract steps 6–8; quickstart V2/V5).

**Checkpoint**: US2 verifiable — run record carries stream_c cost/tokens and specific failure causes; provider scope correct.

## Phase 5: User Story 3 — Insights signal alone produces rules (Priority: P3)

**Goal**: A run where only Stream C has signal still produces rules; all-empty still fails unchanged.

**Independent test**: quickstart V4 — empty obs+git, real session history ⇒ ≥1 rule; all three empty ⇒ unchanged "No streams produced findings" failure.

- [X] T012 [US3] Generalize the Phase-2 synthesis-decision block in `src-tauri/src/learning.rs`: replace the `has_obs`/`has_git` cascade with a uniform rule over the three `Option<StreamFindings>` {obs, git, insights} — 0 non-empty ⇒ unchanged failure (`"No streams produced findings"`, status `failed`, `Err(msg)`); exactly 1 ⇒ use it via `to_analysis_output()` (skip Sonnet) with an accurate `source` label; ≥2 ⇒ `synthesize_findings(...)`; include insights in the `"streams"` `RunPhase` findings count (FR-004; research R-2; contract caller-side).

**Checkpoint**: US3 verifiable — insights-only runs yield rules; negative control unchanged.

## Phase 6: Polish & Cross-Cutting Concerns

- [X] T013 [P] `lat.md/data-flow.md` — **corrected** (initial "N/A" was wrong; a Stop-hook re-check caught it): the Learning Analysis Pipeline omitted Stream C entirely and its lead said only "observations and git history". Updated the lead to include session history, added the Stream C step, and rewrote the synthesis step to the uniform 3-stream decision (0→fail / 1→direct / ≥2→Sonnet). Also fixed the `features.md` Learning System lead paragraph (RAG overview) for the same omission.
- [X] T014 [P] Sync `lat.md/features.md` — Learning System "Stream C extracts session insights via `claude /insights`" → Quill-native local session analysis through the unified inference path.
- [X] T015 [P] `lat.md/backend.md` — verified: the Claude Code Inference Client section did not carry the "StreamC reserved/future" framing in lat.md (that text lived only in the `cc_client.rs` doc comment, refreshed in T009). No lat.md change needed (confirmed via grep). N/A by inspection.
- [X] T016 Run `lat check` from repo root and fix any broken wiki/code refs introduced by T013–T015 (project post-task requirement).
- [X] T017 Run `cargo check --lib` + `cargo test --lib` (existing `cc_client` tests must stay green) and the cleanup sweep `grep -rn "/insights\|InsightsData\|InsightsFacet\|gather_insights" src-tauri/src/` ⇒ no learning-pipeline matches (FR-011).
- [ ] T018 **DEFERRED (manual)** — `quickstart.md` V1–V8 (SC-001…SC-008) require launching the Tauri GUI and triggering live Analyze runs (incl. forced-failure, provider-scoped, seeded-secret cases); cannot be executed from this non-interactive context. Static substitutes already done: cleanup grep (0 matches, FR-011), `cargo check`+`cargo test --lib` green (36/36), `lat check` green. Owner to run V1–V8 against a build of this branch and record deviations here.

---

## Dependencies & Execution Order

- **Setup (T001)** → blocks everything.
- **Foundational (T002, T003 in parallel; T004 after both)** → blocks all user stories.
- **US1 (T005→T006→T008; T007 and T009 in parallel with the chain)** → MVP; blocks US2 & US3 (they build on a working stream).
- **US2 (T010, T011)** and **US3 (T012)** — independent of each other; both require US1.
- **Polish (T013–T018)** — T013/T014/T015 parallel; T016 after them; T017/T018 after all implementation.

## Parallel Execution Examples

- Foundational: `T002` (selection) ∥ `T003` (redaction) — different concerns/files.
- US1: `T009` (cc_client.rs comment) ∥ the `learning.rs` chain.
- Polish: `T013` ∥ `T014` ∥ `T015` — three distinct `lat.md/` files.

## Implementation Strategy

- **MVP = Phase 1 + 2 + 3 (US1)**: delivers the core value — local-only Stream C producing findings, external `/insights` dependency eliminated. Shippable on its own.
- **Incremental**: layer US2 (observability/scope) then US3 (insights-only rule production) — each independently verifiable via its quickstart V-cases.
- **Design-bearing tasks**: T003 (redaction pattern set/masking token) and T004 (digest assembly + budget allocation) carry the meaningful implementation choices flagged in research R-4 — these are the natural points for deliberate decisions at `/speckit-implement`, not mechanical fills.
- **Net surface**: ~190 lines deleted (T008) + one new stream fn + helpers + 4 mechanical seam edits; no `models.rs`/DB/frontend change; zero `cc_client.rs` logic change.
