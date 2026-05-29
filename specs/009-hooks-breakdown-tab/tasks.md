---

description: "Tasks for feature 009 — Hooks Breakdown Tab"
---

# Tasks: Hooks Breakdown Tab

**Input**: Design documents from `/specs/009-hooks-breakdown-tab/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md,
contracts/

**Tests**: The feature spec does not request unit-test tasks; the
quickstart walkthrough is the verification mechanism, supplemented by
the existing `#[cfg(test)]` blocks in storage and sessions modules that
must continue to pass.

**Organization**: Tasks are grouped by user story (US1, US2, US3) so
each can be implemented and verified independently. Foundational tasks
(Phase 2) are blocking prerequisites for every user story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no incomplete
  dependencies)
- **[Story]**: Which user story this task belongs to (US1, US2, US3)
- File paths are absolute relative to the repo root

## Path Conventions

Tauri 2 desktop app: Rust backend under `src-tauri/src/`, TypeScript
frontend under `src/`, managed Codex scripts under
`src-tauri/codex-integration/scripts/`, architecture docs under
`lat.md/`.

## Phase 1: Setup (Shared Infrastructure)

No setup tasks required. The feature is additive — schema, modules,
build tooling, and dependencies already exist on `main`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Land migration 27, the storage primitives, the model
types, the Claude extractor, and the reingest hook. Nothing in
Phase 3+ can start until this phase completes; every user story
depends on these existing.

**⚠️ CRITICAL**: Do not start any US task until T001..T010 are
complete and `cargo build` succeeds.

- [x] T001 Add migration 27 (`hook_invocations` table + UNIQUE
  identity index + four secondary indices +
  `hook_invocation_reingest_pending = '1'`) in
  `src-tauri/src/storage.rs` immediately after the migration 26 block.
  Use `INSERT OR REPLACE INTO settings` and `CREATE TABLE IF NOT
  EXISTS` to keep the migration safely re-runnable. Bump
  `schema_version` to 27. Schema body matches the DDL in
  `specs/009-hooks-breakdown-tab/data-model.md` § Migration 27.
- [x] T002 [P] Add `HookInvocationInput<'a>` and `CodexHookObservation`
  structs in `src-tauri/src/models.rs` next to the existing
  `SessionEventInput<'a>`. Mirror lifetime conventions.
- [x] T003 [P] Add `HookBreakdown` response struct in
  `src-tauri/src/models.rs` with the field set documented in
  `contracts/hook-breakdown-ipc.md` § Response (`hook_identity`,
  `hook_event`, `tool_name`, `is_quill`, `codex_count`,
  `claude_count`, `total_count`, `last_fired_at`). Derive `Serialize`.
- [x] T004 [P] Implement `canonicalize_hook_identity(command,
  hook_name)` helper in `src-tauri/src/sessions.rs` next to
  `skill_access_from_skill_tool_input` per R-D in
  `specs/009-hooks-breakdown-tab/research.md` and the rule in
  `contracts/hook-invocations.md` § Canonicalization rule. Include a
  small `is_quill_managed_path` predicate.
- [x] T005 Implement `Storage::store_hook_invocations_for_messages(
  provider, invocations: &[HookInvocationInput<'_>]) -> Result<(),
  String>` in `src-tauri/src/storage.rs`, modeled on
  `store_skill_usages_for_messages`. Single transaction, `INSERT OR
  IGNORE` against the UNIQUE index, length-cap fields per
  `data-model.md` § Validation rules.
- [x] T006 [P] Implement `Storage::store_codex_hook_observation(obs:
  CodexHookObservation) -> Result<(), String>` in
  `src-tauri/src/storage.rs`. Same insert path as T005 but takes a
  single observation; reused by the HTTP endpoint background task.
- [x] T007 [P] Implement `Storage::delete_hook_invocations_for_session(
  provider, session_id) -> Result<(), String>` in
  `src-tauri/src/storage.rs`. Single `DELETE` matching the
  `delete_skill_usages_for_session` shape.
- [x] T008 Wire `delete_hook_invocations_for_session` into the
  existing per-session, per-host, and per-project cascades in
  `src-tauri/src/storage.rs`: add the call alongside the skill_usages
  block inside `delete_session_data`, `delete_host_data`, and
  `delete_project_data`. Also add a `DELETE FROM hook_invocations
  WHERE cwd = ?1` next to the existing cwd-based skill cleanup.
- [x] T009 Implement `Storage::get_hook_breakdown(days, provider,
  all_time, limit) -> Result<Vec<HookBreakdown>, String>` in
  `src-tauri/src/storage.rs`. Signature mirrors `get_skill_breakdown`;
  SQL produces `is_quill` via `CASE WHEN hook_identity LIKE 'quill:%'`
  and orders by `total_count DESC, hook_identity ASC` per the contract.
- [x] T010 Implement `extract_hook_invocations_from_attachment(record,
  session_cwd, hostname)` in `src-tauri/src/sessions.rs`. Returns
  `Option<HookInvocationInput>`, gated on `record.type == "attachment"`
  and `attachment.type` starting with `hook_`. Field mapping per
  `contracts/hook-invocations.md` § Field mapping (Claude). Use
  `canonicalize_hook_identity` from T004 for `hook_identity`.
- [x] T011 Wire the extractor from T010 into the existing dual-emission
  pass in `src-tauri/src/sessions.rs` (the `process_discovered_file`
  / message-walk site, alongside `ExtractedMessage` and
  `ExtractedEvent` construction). Collect into a per-batch
  `Vec<HookInvocationInput>` and call
  `storage.store_hook_invocations_for_messages` once per batch.
  Mirror the warn-and-continue error handling used for the skill
  usage path.
- [x] T012 Extend the post-boot reingest sweep at the bottom of
  `src-tauri/src/sessions.rs` to read
  `hook_invocation_reingest_pending`, replay the attachment
  extractor across every Claude JSONL when set, and clear the flag on
  clean completion. Add the new branch next to the existing
  `runtime_event_reingest_pending` handling so the three flags share
  one sweep.

**Checkpoint**: Migration 27 applies cleanly. `cargo build` succeeds.
`cargo test` passes (modules still build with new symbols). The new
table is empty and indexes are present.

---

## Phase 3: User Story 1 - See hook firings at a glance (Priority: P1) 🎯 MVP

**Goal**: User opens Analytics → Now → Hooks and sees rows reflecting
hook fires from both Claude (live + backfilled) and Codex (live), with
counts sorted descending and a "last fired" relative timestamp on each
row.

**Independent Test**: With a fresh Quill build (Phase 2 applied), open
a Claude session that fires at least three distinct hook scripts and a
Codex session that fires a user prompt plus a Bash tool. After ingest
completes, the Hooks breakdown lists rows for both providers, sorted
by Uses descending, and counts increment as more events fire (SC-001,
SC-002).

### Implementation for User Story 1

- [x] T013 [US1] Register `get_hook_breakdown` as a Tauri command in
  `src-tauri/src/lib.rs` next to `get_skill_breakdown`. Add to the
  `invoke_handler` macro list and the async wrapper that calls
  `run_blocking`.
- [x] T014 [US1] [P] Implement `post_hook_observed` handler in
  `src-tauri/src/server.rs`. Route `POST /api/v1/hooks/observed` to
  it, add to the router builder near `post_observation`. Validate
  fields per `contracts/hooks-observed-endpoint.md` § Wire format,
  fast-ack with `202 Accepted`, dispatch
  `Storage::store_codex_hook_observation` on a blocking task. Emit
  a Tauri event `hooks-observed-updated` after a successful insert.
- [x] T015 [US1] [P] Create
  `src-tauri/codex-integration/scripts/hook-observe.cjs` per
  `contracts/hooks-observed-endpoint.md` § Producer. ≤ 80 lines, no
  new deps, exits 0 on any error, honors `QUILL_DEBUG`. Mark
  executable (mode 0o755) on installation.
- [x] T016 [US1] In `src-tauri/src/integrations/codex.rs`:
  (1) add `"hook-observe.cjs"` to `ALL_MANAGED_SCRIPT_FILES`;
  (2) add helper `hook_observation_scripts_for(features)` returning
  `vec!["hook-observe.cjs"]` when `features.activity_tracking` is
  true and empty otherwise;
  (3) extend the hook-group builder so when activity_tracking is on
  it produces eight new `CodexHookGroup` entries (one per event in
  `CODEX_HOOK_EVENTS`), each with a single `CodexHookCommand` whose
  `command = node "<absolute hook-observe.cjs path>"` and
  `timeout = 3`;
  (4) ensure orphan removal logic removes these eight blocks when
  activity_tracking flips off.
- [x] T017 [US1] [P] Add `useHookBreakdown` to
  `src/hooks/useBreakdownData.ts` per `contracts/hook-breakdown-ipc.md`
  § Frontend consumer. Follow the existing state pattern; subscribe
  to both `skill-usages-updated` and `hooks-observed-updated` events;
  1-second debounce; 60-second polling interval.
- [x] T018 [US1] [P] Add `HookBreakdownRow` type in `src/types.ts`
  with the camelCase field set; add boundary mapper
  `fromHookBreakdownPayload` for the snake_case → camelCase
  conversion at the IPC boundary.
- [x] T019 [US1] Extend `src/components/analytics/BreakdownPanel.tsx`
  to support a new `'hooks'` mode: add the tab in the mode selector
  (positioned after `'skills'`), wire `useHookBreakdown`, and render
  one row per identity with columns Hook / Uses / Last used. Sort
  client-side by total count desc with tie-break on `hookIdentity`
  ascending. Reuse the existing relative-timestamp helper used by the
  Skills "Last used" column.
- [x] T020 [US1] Wire the new Hooks tab into the selector in
  `src/components/analytics/NowTab.tsx` so it appears alongside
  Sessions / Projects / Hosts / Skills, in that order.

**Checkpoint**: `npm run tauri dev`. Open a Claude session, fire a few
prompts and tool calls. Open Analytics → Now → Hooks. See rows for
Claude script identities. Start a Codex session, fire prompts. Within
~2 seconds see Codex event rows appear. Quickstart Steps 3–4 pass.

---

## Phase 4: User Story 2 - Distinguish Quill telemetry from user hooks (Priority: P2)

**Goal**: Rows backed by Quill-deployed scripts keep the `quill:`
identity prefix, distinguishable from plugin-installed or personal
hooks without a separate badge.

**Independent Test**: With Phase 3 complete and Quill's
`activity_tracking` ON, the Hooks breakdown shows `quill:` in every
row whose `hookIdentity` starts with that prefix; non-Quill rows do not
show that prefix (SC-004).

### Implementation for User Story 2

- [x] T021 [US2] [P] Keep the `isQuill`/`is_quill` field in the
  contract so callers can classify Quill-managed rows when needed.
- [x] T022 [US2] In `src/components/analytics/BreakdownPanel.tsx`,
  render the `hookIdentity` label directly, preserving the `quill:`
  prefix in row text without adding a separate QUILL badge.

**Checkpoint**: Open Hooks breakdown. Rows starting `quill:` show that
prefix in the identity text; plugin and personal rows do not.
Quickstart Step 4 passes.

---

## Phase 5: User Story 3 - Scope by provider and by lifetime (Priority: P3)

**Goal**: User can filter the Hooks breakdown by provider via the
All/Codex/Claude strip and toggle ALL TIME via the `∞` chip, matching
the controls on the Skills breakdown. A header help affordance explains
the Claude/Codex asymmetry.

**Independent Test**: With Phase 4 complete, switching the provider
strip changes the visible row set; activating ALL TIME shows rows
across all indexed history regardless of the Now-tab timeframe; the
help affordance opens a tooltip with the FR-017 copy (SC-005).

### Implementation for User Story 3

- [x] T023 [US3] Reuse the provider filter strip already used by the
  Skills breakdown for the Hooks breakdown in
  `src/components/analytics/BreakdownPanel.tsx`. Wire its selection
  to the `provider` argument passed to `useHookBreakdown`. When
  `'claude'` selected, display `claudeCount` per row and hide rows
  with `claudeCount === 0`; same logic for `'codex'`; on `'all'`,
  display `totalCount`.
- [x] T024 [US3] [P] Reuse the existing `∞ ALL TIME` chip component
  for the Hooks breakdown, wiring its toggle to the `allTime`
  argument passed to `useHookBreakdown`.
- [x] T025 [US3] [P] Add a `?` help affordance on the Hooks breakdown
  header in `src/components/analytics/BreakdownPanel.tsx`, identical
  in styling to the `InsightCard` `?` button. Tooltip copy (matching
  FR-017): "Claude hooks are tracked per script. Codex hooks are
  tracked per event because Codex doesn't log per-script hook
  executions."

**Checkpoint**: Quickstart Step 5 passes; Step 3 reconfirmed with the
filter strip and ALL TIME chip exercised.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, code-reference comments, end-to-end
verification, and quality gates.

- [x] T026 Update `lat.md/backend.md`:
  (1) add a "Hook Invocations" subsection under
  `Database#Schema` describing the new table, indices, and migration
  27;
  (2) add `POST /api/v1/hooks/observed` to the Endpoints table;
  (3) add `get_hook_breakdown` to the Tauri IPC commands listing.
- [x] T027 [P] Update `lat.md/data-flow.md`: extend the
  `Session Indexing Pipeline` section to note that hook attachment
  records are extracted alongside messages and events in the
  dual-emission pass.
- [x] T028 [P] Update `lat.md/features.md`: extend the
  `Analytics Dashboard#Now Tab` description to mention the Hooks
  breakdown row and the Claude/Codex granularity help affordance.
- [x] T029 [P] Update `lat.md/infrastructure.md`: extend the
  `Codex Integration Deployment` section to note `hook-observe.cjs`
  and its eight-event registration, gated on `activity_tracking`.
- [ ] T030 [P] Update `lat.md/tests.md`: add test-spec sections
  covering hook extraction, the new endpoint, the IPC command, and
  the breakdown query. Reference each from the new `// @lat:`
  comments planted in T031.
- [x] T031 Plant `// @lat:` references in source for the new
  surfaces: `src-tauri/src/storage.rs` (migration 27 block + storage
  primitives), `src-tauri/src/sessions.rs` (extractor + reingest
  sweep), `src-tauri/src/server.rs` (post_hook_observed handler),
  `src-tauri/src/integrations/codex.rs` (hook-observe deployment).
  Each reference points to a section authored in T026–T030.
- [x] T032 Run the quickstart walkthrough
  (`specs/009-hooks-breakdown-tab/quickstart.md`) end-to-end on a
  development machine. Steps 1–9 must all reach their Pass criteria.
- [x] T033 Run `cargo test --manifest-path src-tauri/Cargo.toml`,
  `npm run lint`, and `npx lat check`. All three must pass with no
  new warnings beyond the existing baseline.

---

## Dependencies & Execution Order

### Phase dependencies

- Phase 1 (none) → trivially complete.
- Phase 2 (T001–T012) must complete before any of Phase 3, 4, 5, 6.
- Phase 3 (T013–T020) implements the MVP. Phases 4 and 5 each consume
  the data and components built in Phase 3.
- Phase 4 (T021–T022) depends on Phase 3 (BreakdownPanel rendering
  rows already).
- Phase 5 (T023–T025) depends on Phase 3 (filter strip wiring needs
  the existing renderer) but is independent of Phase 4.
- Phase 6 (T026–T033) depends on all prior phases (docs reflect
  shipped behavior; quickstart exercises the whole feature).

### Critical path

T001 → T010 → T011 → T012 → T013 → T019 → T020 → T032

This is the minimum chain to demonstrate the feature end-to-end on
Claude data alone.

### Parallel opportunities

Inside Phase 2: T002, T003, T004, T006, T007 can all run in parallel
once T001 lands.

Inside Phase 3: T014 (server endpoint), T015 (codex script file),
T017 (frontend hook), T018 (types) can all run in parallel after
T013 and the Phase-2 storage primitives exist.

Inside Phase 6: T027, T028, T029, T030 are all independent doc edits
on different `lat.md/` files.

## Implementation Strategy

### MVP slice (ship this first)

Phases 2 + 3 only. That gives a working Hooks breakdown with both
provider data sources flowing. Users see the feature, all FR-001
through FR-017 are satisfied (excluding the Quill-managed identity
distinction and asymmetry tooltip, which are FR-006 and FR-017
deferred to Phases 4 and 5 respectively).

### Incremental delivery

- After Phase 2: Migration runs; reingest backfills historical Claude
  hooks; no UI yet (foundation only).
- After Phase 3: MVP visible to user.
- After Phase 4: Telemetry honesty (users distinguish Quill rows from
  their own).
- After Phase 5: Provider scoping and ALL TIME parity with Skills.
- After Phase 6: Docs in sync, lat check passes, quickstart verified.

### Parallel team strategy

Two engineers could split as follows after Phase 2:

- Engineer A: Phase 3 backend (T013, T014, T015, T016) → Phase 6
  backend docs (T026, T031).
- Engineer B: Phase 3 frontend (T017, T018, T019, T020) → Phase 4
  (T021, T022) → Phase 5 (T023, T024, T025) → frontend docs (T028,
  T030).

## Notes

- The reingest flag does the heavy lifting for historical Claude
  data. No manual backfill required.
- Codex side starts at zero rows on first launch and accrues over
  time; this is documented behavior, not a bug.
- The migration is additive only — no existing tables touched; no
  data migration; rollback is a single `DROP TABLE` + version
  cleanup as described in quickstart § Rollback.
- The new endpoint reuses the bearer-auth + rate-limit middleware
  already applied to every other `/api/v1` route.
