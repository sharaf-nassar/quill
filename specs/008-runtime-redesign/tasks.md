---
description: "Task list for active-time runtime tracking redesign"
---

# Tasks: Active-Time Runtime Tracking Redesign

**Input**: Design documents from `/specs/008-runtime-redesign/`
**Prerequisites**: plan.md, spec.md (US1..US4), research.md (R-A..R-F),
data-model.md (migration 26 + session_events schema),
contracts/session-events.md (EVT-CL-*, EVT-CX-*, ING-*, IDX-*),
contracts/llm-runtime-stats.md (STAT-1..7, UI-1..3)

**Tests**: Not requested by the user and not required by the repo
convention (CLAUDE.md: "Write test code only when the user explicitly
requests it"). Existing `#[cfg(test)]` blocks in `storage.rs` and
`sessions.rs` will be exercised by `cargo test` as part of polish; new
test code is out of scope for this task list.

**Organization**: Tasks are grouped by user story (US1..US4 from
spec.md). Foundational tasks complete the migration + ingest plumbing
that every story depends on. US1 (the headline runtime card) is the
MVP slice; US2 (sub-agent attribution) is a tight follow-on on the same
data path. US3 (backfill) and US4 (copy) bring the feature to
production parity.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Maps task to spec.md user story (US1..US4)
- Paths are absolute repo-relative

---

## Phase 1: Setup

The feature lives inside the existing Tauri 2 + React project. No
scaffolding is required — the redesign extends `storage.rs`,
`sessions.rs`, `models.rs`, and `NowTab.tsx` plus four lat.md files.
Skip this phase.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Land the schema, struct types, and storage primitives that
every user story builds on. Nothing in Phase 3+ can begin until this
phase completes (R-D obligations require the table + ingest API to
exist).

**⚠️ CRITICAL**: Do not start any US task until all of T001..T006 are
complete and `cargo build` succeeds.

- [x] T001 Add migration 26 (`session_events` table + three indices +
  `runtime_event_reingest_pending = '1'`) in
  `src-tauri/src/storage.rs` immediately after the migration 25 block.
  Use `INSERT OR REPLACE INTO settings` and `CREATE TABLE IF NOT
  EXISTS` to keep the migration safely re-runnable. Bump
  `schema_version` to 26.
- [x] T002 [P] Add `SessionEventKind` enum (5 variants) and
  `ExtractedEvent` struct in `src-tauri/src/sessions.rs` near the
  existing `ExtractedMessage` definition. Add the new `events` field to
  `ExtractedSession` (default empty vec for backward compatibility
  during incremental builds).
- [x] T003 [P] Add `SessionEventInput<'a>` struct in
  `src-tauri/src/storage.rs` next to the existing
  `ResponseTimeInput<'a>` so the ingestion API surface stays grouped.
  Mirror its lifetime conventions.
- [x] T004 Implement `Storage::ingest_session_events(provider,
  session_id, events: &[SessionEventInput<'_>]) -> Result<(), String>`
  in `src-tauri/src/storage.rs` per contract ING-1..ING-5 (empty
  short-circuit, sort by timestamp, single transaction,
  `INSERT OR IGNORE`, RFC3339 parse with warn-on-fail).
- [x] T005 [P] Implement `Storage::delete_session_events_for_session(
  provider, session_id) -> Result<(), String>` in
  `src-tauri/src/storage.rs`. Single `DELETE FROM session_events
  WHERE provider = ?1 AND session_id = ?2`.
- [x] T006 Wire `delete_session_events_for_session` into the existing
  session-delete cascades in `src-tauri/src/storage.rs`: add the call
  next to the `response_times` / `tool_actions` / `skill_usages` /
  `token_snapshots` block inside `delete_session_data`,
  `delete_host_data`, and `delete_project_data` (the same `for table
  in [...]` loop pattern at lines 9116 and 9149).

**Checkpoint**: Migration 26 applies cleanly; the table exists; the
ingest and delete primitives compile and pass `cargo build`.

---

## Phase 3: User Story 1 - Trustworthy active-time metric (Priority: P1) 🎯 MVP

**Goal**: The Now tab's LLM Runtime card reports total active time
that matches the user's lived experience — model + tool time
counted, user-idle gaps excluded.

**Independent Test**: After Phase 3, on a developer machine with a
heavy CC session active in the last hour, the card's 1h total is
materially larger than the pre-change baseline and within the
documented tolerance of the wall-clock active time independently
measured from the transcript (SC-001).

### Implementation for User Story 1

- [x] T007 [US1] Extend `extract_claude_messages_from_jsonl` in
  `src-tauri/src/sessions.rs` to populate `ExtractedSession.events`.
  Walk the same JSONL lines as today; for each `user`/`assistant`
  line with a non-empty timestamp and `isMeta != true`, classify the
  event kind per R-C / EVT-CL-1..EVT-CL-7 and push an
  `ExtractedEvent`. Read `isSidechain`, `agentId`, `uuid`, and
  `parentUuid` exactly as the existing `ExtractedMessage` path does.
- [x] T008 [P] [US1] Extend `extract_codex_messages_from_jsonl` in
  `src-tauri/src/sessions.rs` to emit `user_text` and `asst_text`
  events per EVT-CX-1 (`is_sidechain = false`, `agent_id = None`,
  same `uuid`/`parent_uuid` semantics as the Claude path where
  applicable).
- [x] T009 [US1] In `process_discovered_file`
  (`src-tauri/src/sessions.rs` around lines 638-660 where
  `ingest_response_times` is called), build an `rt_events: Vec<
  SessionEventInput<'_>>` from `extracted.events` and call
  `storage.delete_session_events_for_session(...)` followed by
  `storage.ingest_session_events(...)`. Mirror the existing
  warn-and-continue error handling.
- [x] T010 [US1] Rewrite `Storage::get_llm_runtime_stats` in
  `src-tauri/src/storage.rs` (around line 8742) to source from
  `session_events`. Implement STAT-1..STAT-7 from the contract:
  RFC3339 cutoff, chain-scoped walk, tool-loop gap exception via
  `IDLE_THRESHOLD_SECS = 300.0` and `TOOL_WAIT_MAX_SECS = 21_600.0`,
  sparkline 7-bucket distribution, clamp negative gaps to zero. Keep
  the `LlmRuntimeStats` return shape identical so the IPC contract
  is unchanged.

**Checkpoint**: `npm run tauri dev`; open Now tab; LLM Runtime card
total has grown materially. Range switch (1h/24h/7d/30d) updates
both total and sparkline. `parent_only` scope still excludes
sub-agents (T012 verifies further).

---

## Phase 4: User Story 2 - Sub-agent attribution stays correct (Priority: P1)

**Goal**: Sub-agent chains contribute their own active time without
double-counting the parent's wait, the `parent_only` scope continues
to work, and the existing Sessions breakdown + sub-agent tree
(backed by `response_times`) still render correctly.

**Independent Test**: Expand a session with at least one sub-agent
in the Sessions breakdown. Parent row turn_count > 0, sub-agent rows
have their own windows, headline card total is consistent with the
union (not the sum) of parent + sub-agent active intervals
(SC-006 + scenario US2.1).

### Implementation for User Story 2

- [x] T011 [US2] Verify in `Storage::get_llm_runtime_stats` that
  rows are ordered by `(provider, session_id, COALESCE(agent_id,''),
  timestamp)` and the chain key includes `agent_id`. Add a
  `cargo test` assertion under the existing storage `#[cfg(test)]`
  block at `src-tauri/src/storage.rs` that two sibling sub-agents
  (same parent message, different `agent_id`) yield two independent
  turns rather than one stitched turn.
- [x] T012 [US2] Confirm the `scope = "parent_only"` branch of
  `Storage::get_llm_runtime_stats` selects `WHERE is_sidechain = 0`
  against `session_events` (STAT-2 bracket clause); add a `cargo
  test` assertion that excluding sub-agents produces a smaller
  total than the `all` scope on a fixture with mixed parent + sub-
  agent rows.
- [x] T013 [P] [US2] Run an end-to-end check against
  `src-tauri/src/storage.rs::get_session_subagent_tree` and
  `get_session_breakdown` — these still read from
  `response_times` (intentional, per R-A). Run `cargo test
  storage::tests::get_session_subagent_tree` and
  `storage::tests` for `get_session_breakdown` to confirm neither
  regresses.

**Checkpoint**: All three storage tests pass; the Now tab Sessions
breakdown row expand works; `parent_only` scope produces a smaller
total than `all`.

---

## Phase 5: User Story 3 - Historical data backfills automatically (Priority: P2)

**Goal**: On first launch after upgrade, the existing mtime-based
session indexer sweeps every transcript on disk and populates
`session_events` without any user action.

**Independent Test**: On a machine with at least 100 historical
transcripts, fresh install of the new build, no manual interaction —
within minutes of launch, the Now tab's 24h LLM Runtime total
reflects redesigned semantics applied to historical data (SC-004).

### Implementation for User Story 3

- [x] T014 [US3] In `src-tauri/src/sessions.rs`, locate the boot-time
  handler that honors `subagent_reingest_pending` and
  `skill_usage_reingest_pending`. Add an analogous branch for
  `runtime_event_reingest_pending`: when the setting reads `'1'`,
  clear `index_state.json::file_mtimes` for both providers and
  clear the flag (`UPDATE settings SET value = '0'`).
- [x] T015 [US3] Confirm the standard mtime sweep at the next tick
  (existing notify-driven path) re-runs `process_discovered_file`
  for every transcript, which now calls T009's
  `ingest_session_events` — no new background task or scheduler is
  introduced (assumption #6).

**Checkpoint**: Manual test per `quickstart.md` step 2 — table
populates progressively; no user click required; flag clears after
sweep starts.

---

## Phase 6: User Story 4 - Card description matches the math (Priority: P3)

**Goal**: The InsightCard help tooltip on the LLM Runtime card
truthfully describes the new semantics (active time including tool
execution; user-idle excluded).

**Independent Test**: Click the `?` icon on the card; tooltip text
matches UI-2 from `contracts/llm-runtime-stats.md`.

### Implementation for User Story 4

- [x] T016 [US4] Update the `description` prop of the LLM Runtime
  `InsightCard` in `src/components/analytics/NowTab.tsx` (around
  line 191) to the UI-2 string from
  `specs/008-runtime-redesign/contracts/llm-runtime-stats.md`. No
  other React/TypeScript change is required (UI-1, UI-3).

**Checkpoint**: Tooltip displays the new wording in dev build.

---

## Phase 7: Polish & Cross-Cutting Concerns

- [x] T017 [P] Update `lat.md/backend.md` per R-F: add a
  `session_events` bullet under `#Schema` between
  `#Code and Runtime Metrics` and `#Metadata`; amend `#Metadata`
  to note migration 26 and the `runtime_event_reingest_pending`
  flag; update `#Tauri IPC Commands#Code and Response Stats (5)`
  to describe `get_llm_runtime_stats` sourcing from
  `session_events` with R-B semantics.
- [x] T018 [P] Update `lat.md/data-flow.md` per R-F: amend the
  `#Session Indexing Pipeline` subsection to describe dual emission
  (`messages` + `events`) from `extract_*_messages_from_jsonl`.
- [x] T019 [P] Update `lat.md/features.md` per R-F: amend
  `#Analytics Dashboard#Now Tab` LLM Runtime sentence to describe
  active time (model + tool execution) with user-idle gaps over
  5 minutes excluded.
- [x] T020 Add `@lat:` source-code refs to the new functions
  (`Storage::ingest_session_events`,
  `Storage::delete_session_events_for_session`,
  `Storage::get_llm_runtime_stats` after rewrite, the new
  `ExtractedEvent` and `SessionEventKind`, the
  `runtime_event_reingest_pending` handler) where appropriate so
  `lat check` link validation passes.
- [x] T021 Run `lat check` from the repo root and resolve any
  failures introduced by T017..T020.
- [x] T022 Run `cargo test` in `src-tauri/`. Failures in the
  existing `storage::tests::response_times_*` or
  `sessions::tests::*` suite indicate an inadvertent regression
  (T013 catches the sub-agent-tree path explicitly; this is the
  broad sweep).
- [ ] T023 Execute `specs/008-runtime-redesign/quickstart.md`
  steps 1-8 against a clean install. Capture before/after screenshots
  of the LLM Runtime card for the 1h window. Step 8 verification
  (cargo test + lat check) is implied by T021 and T022 but the
  quickstart's other steps must be exercised manually.
- [ ] T024 Open the dev build, type a few prompts in CC that
  trigger long tool runs and ones that include an idle pause >5
  minutes; eyeball the card total against the documented
  semantics. This is the SC-002 + SC-003 manual confirmation that
  no fixture can substitute for.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: skipped; no scaffolding required.
- **Foundational (Phase 2)**: must complete before any US task.
  Within Phase 2: T001 blocks T004-T006 (the table must exist
  before delete/ingest can be wired); T002, T003, T005 can run
  in parallel after T001; T004 depends on T003.
- **US1 (Phase 3)**: must complete first — it delivers the MVP and
  contains the rewrite of `get_llm_runtime_stats` that the rest
  of the feature relies on.
- **US2 (Phase 4)**: depends on US1 (rewritten query and event
  ingestion). T011 + T012 may be developed in parallel; T013 is
  independent and parallelizable.
- **US3 (Phase 5)**: only depends on Phase 2 (the flag is set by
  T001). T014 and T015 are essentially a single edit point and
  its observation.
- **US4 (Phase 6)**: only depends on US1 landing (so the new
  semantics are real). Single-line frontend change.
- **Polish (Phase 7)**: depends on US1..US4. T017-T020 can run in
  parallel; T021-T024 are gates and run sequentially.

### Critical Path

`T001 → T004 → T007 → T010 → T011 → T014 → T017 → T021 → T023`

### Parallel Opportunities

Within Phase 2: `T002 || T003 || T005` after T001.
Within US1: `T007 || T008` are different extractor branches in the
same file but on independent code paths; treat as serial by default
to avoid merge friction, parallel if working from two worktrees.
Within US2: `T013` runs in parallel with T011/T012.
Within Polish: `T017 || T018 || T019` are three independent lat.md
edits.

---

## Implementation Strategy

### MVP slice (ship this first)

1. Complete Phase 2 (foundation).
2. Complete Phase 3 (US1 — the headline card now tells the truth).
3. Validate per `quickstart.md` steps 1-4. This is a shippable
   improvement on its own.

### Incremental delivery

1. MVP slice above.
2. Add Phase 4 (US2 — sub-agent attribution correctness).
3. Add Phase 5 (US3 — backfill flag, so historical totals fill in).
4. Add Phase 6 (US4 — copy fix, ships with the rest because UI
   work is tiny).
5. Run Phase 7 polish + lat.md updates + run quickstart to validate
   end-to-end.

### Parallel team strategy

With two developers: one drives T001 → T004 → T007 → T009 → T010
(the storage + extractor core path); the other prepares T002, T003,
T005, T006, T008 in parallel and joins on T011/T012 once T010 is
in.

---

## Notes

- All new SQL goes through `prepare_cached` + `params!` exactly like
  the existing `response_times` code in `storage.rs`. Do not invent
  a new helper.
- The IPC surface (`get_llm_runtime_stats(range, scope?)`) is
  unchanged; `useLlmRuntimeStats.ts` and the Recharts sparkline
  consumer need no edit.
- The existing `response_times` table is intentionally untouched.
  Do not drop it, do not alter its schema, do not rewrite its
  queries — that is scope for a future feature.
- `cargo test` and `lat check` are the merge gates; quickstart.md
  is the human gate.
- Commit boundaries match phase boundaries (one commit per
  completed phase is the lower bound; per task is fine too).
