# Tasks: Session Model Analytics

**Input**: Design documents from `/specs/011-session-model-analytics/`

**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`,
`contracts/`, and `quickstart.md`

**Tests**: No automated test-code tasks are generated because project policy
requires explicit user authorization. Story checkpoints use isolated manual
fixtures and existing repository checks only.

**Organization**: Tasks are grouped by user story so each priority delivers a
usable increment. Run `lat check` after every implementation task and before
reporting that task complete.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Safe to execute in parallel after its stated prerequisites because it
  touches different files and does not depend on unfinished implementation.
- **[Story]**: Maps the task to US1, US2, or US3 from `spec.md`.
- Every checklist item names its exact implementation or validation path.

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Establish the feature module and contract-aligned types without
adding provider SDKs, model catalogs, or model-specific dependencies.

- [X] T001 Create the `model_usage` module boundary and event/state constants in `src-tauri/src/model_usage.rs`, then declare the module in `src-tauri/src/lib.rs`
- [X] T002 [P] Define camelCase Rust DTOs for model identity, ranges, analytics rows, history points, root-complete backfill status, paged sessions, chain segments, update events, and the serialized `{ code, message }` `ModelAnalyticsError` envelope in `src-tauri/src/models.rs`
- [X] T003 [P] Define matching frontend analytics, history, paging, chain, root-complete backfill, and structured error types, an unambiguous provider/model identity key, and add `models` to `AnalyticsTab` in `src/types.ts`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Build model-neutral evidence parsing and source-scoped persistence
required by every user story.

**⚠️ CRITICAL**: No user story work begins until this phase is complete.

- [X] T004 Add migration 28 with `model_usage_observations`, root-owned `model_observation_sources`, root-counted `model_backfill_state`, constraints, query indexes, source uniqueness, and the pending singleton plus read-only `Storage::get_model_backfill_status`; use explicit transactional child deletion without changing SQLite's global foreign-key pragma in `src-tauri/src/storage.rs`
- [X] T005 [P] Expose `pub(crate)` Claude and Codex provider-root enumeration with stable root keys, completion/failure outcomes, canonical source paths/keys, filesystem provenance, path-layout hints, and demo-aware data-path resolvers—without parsing transcript-native session metadata—in `src-tauri/src/sessions.rs`
- [X] T006 [P] Implement normalized observation/source structs, bounded diagnostics, exact Unicode-trimmed 1–256-scalar model-ID validation using Rust `.chars().count()`, control-character rejection, independently nullable `0..=100_000_000` token validation that preserves valid siblings, and stable record keys in `src-tauri/src/model_usage.rs`
- [X] T007 Implement Claude `assistant` extraction with adapter-owned native session/parent/subagent metadata, explicit raw model evidence, direct independently validated token dimensions, malformed JSON/unsupported shape/missing-timestamp isolation, continued later-record processing, and null-model turn gaps in `src-tauri/src/model_usage.rs`
- [X] T008 Implement Codex adapter-owned `session_meta`, `turn_context`, and `event_msg/token_count` metadata/extraction with malformed JSON/unsupported shape/missing-timestamp isolation and continued later-record processing; emit tokenless model turns and per-dimension cumulative baselines where first/post-decrease values count from zero, monotonic values emit deltas, missing/invalid dimensions preserve baselines, all-zero records emit no row, decreases log bounded reset warnings only, cumulative objects exclude `last_token_usage`, unprovably unique last-only dimensions remain absent, and nearby models are never inferred in `src-tauri/src/model_usage.rs`
- [X] T009 Implement root-owned source inventory lookup, nanosecond-mtime/size/SHA-256 fingerprints, metadata-change detection, atomic `replace_model_source`, last-known-good failure marking, explicit child-before-parent pruning limited to each completed root key, and suppression that clears only inside a successful replacement transaction in `src-tauri/src/storage.rs`
- [X] T010 Build the shared source reconciliation coordinator with parse-before-transaction behavior, cross-source root-graph resolution after adapter parsing, path hints subordinate to native metadata, forced fan-out when resolved parent/root metadata changes, a process-wide runner guard, and immediate post-commit `model-analytics-updated` emission in `src-tauri/src/model_usage.rs`
- [X] T011 Register managed model-usage runner plus source-keyed live-queue state and nonblocking execution plumbing without starting history work from Session Search in `src-tauri/src/lib.rs`

**Checkpoint**: Dynamic provider-qualified observations can be parsed and replaced
per source without changing existing token/session totals or rebuilding unrelated
indexes.

---

## Phase 3: User Story 1 — Identify Models Consuming Tokens (Priority: P1) 🎯 MVP

**Goal**: Show current model-attributed totals and history for every accepted raw
identifier, with honest unattributed coverage and provider context.

**Independent Test**: Process current activity from at least two raw model IDs and
confirm Models shows provider-qualified rows, token dimensions, turns, sessions,
cache-read share, first/last seen, sortable columns, selected-model history, and
0% coverage for an entirely unattributed provider.

### Implementation for User Story 1

- [X] T012 [US1] Implement all-retained global distinct-session scope plus active-range/provider sessions and represented providers from actual normalized timestamps in `[rangeStart, rangeEnd)` joined to unsuppressed source ownership—never first/last interval overlap—with observation-derived evidence count, trustworthy-final scope, entirely unattributed providers, unbounded provider-qualified models per session, exact coverage, distinct/multi-model counts, complete Unicode-scalar/BINARY-ordered rows, totals/turns/sessions/first/last seen, and nullable cache-read share in `src-tauri/src/storage.rs`
- [X] T013 [US1] Implement zero-filled 5-minute/1-hour/6-hour/1-day attributed, unattributed, and selected-model history buckets for `1h`/`24h`/`7d`/`30d` in `src-tauri/src/storage.rs`
- [X] T014 [P] [US1] Invoke source fingerprint reconciliation independently of `IndexState.file_mtimes` from `SessionIndex::startup_scan`, preserving typed read failures and source-scoped ownership in `src-tauri/src/sessions.rs`
- [X] T015 [P] [US1] Admit every validated `/sessions/notify` transcript path to the model queue before existing session-keyed Tantivy coalescing, coalesce model work only by provider/canonical source key, run it despite empty extracted search messages, preserve search behavior, and exclude direct `/sessions/messages` payloads in `src-tauri/src/server.rs`
- [X] T016 [US1] Implement and register `get_model_analytics` and `get_model_history` with range/provider/model validation and the shared serialized bounded `ModelAnalyticsError` envelope in `src-tauri/src/lib.rs`
- [X] T017 [P] [US1] Create `useModelAnalytics` with independent aggregate/history initial/refresh/error state and Retry callbacks, request-identity guards, a fixed one-second coalescing window whose deadline starts at the first `model-analytics-updated` event, a 60-second fallback poll, a shared frontend-only refresh generation for detail hooks, retained same-scope successful data, and React Strict Mode-safe listener/timer cleanup in `src/hooks/useModelAnalytics.ts`
- [X] T018 [P] [US1] Build the compact coverage, distinct-model, and multi-model summary rail with tabular numerics and unavailable coverage semantics in `src/components/analytics/models/ModelSummaryStrip.tsx`
- [X] T019 [P] [US1] Build the labeled aggregate attributed/unattributed history chart with fixed range axes, a visually hidden semantic bucket table containing bounds and every series value, and a signal-blue selected-model overlay explicitly labeled as selection—not model identity—that never hides aggregate coverage in `src/components/analytics/models/ModelUsageHistory.tsx`
- [X] T020 [P] [US1] Build a semantic complete model table with provider badges, neutral selectable/copyable raw IDs beside native inspect buttons using `aria-pressed`, attributed total/share, all token dimensions, turns, sessions, cache-read share, first/last seen, stable sorting for every column, `aria-sort`, and horizontal overflow in `src/components/analytics/models/ModelUsageTable.tsx`
- [X] T021 [P] [US1] Add an always-present Models tab with explicit `tablist`/`tab` roles, stable tab/panel IDs, `aria-controls`, `aria-selected`, roving `tabIndex`, Arrow/Home/End keyboard navigation, five-tab overflow, and no per-model color in `src/components/analytics/TabBar.tsx`
- [X] T022 [US1] Compose separately labeled native-button range/provider groups using `aria-pressed`, All plus response-driven represented providers including entirely unattributed providers, provider-qualified selection reconciliation, independently loading summary/history/table regions, and request-local aggregate/history error notices with Retry actions in `src/components/analytics/ModelsTab.tsx`
- [X] T023 [US1] Add Models range state/routing; wrap every rendered Now, Trends, Charts, Models, and Context surface—including active snapshot-empty content—in the matching `tabpanel` with stable `id`/`aria-labelledby`; then restrict the global snapshot-empty gate to Now, Trends, and Charts in `src/components/analytics/AnalyticsView.tsx`
- [X] T024 [P] [US1] Refactor pane composition so `AnalyticsView` remains mounted without enabled live providers while provider loading/empty/`UsageDisplay` behavior and live polling remain isolated to the live pane in `src/App.tsx`
- [X] T025 [US1] Add Glass Cockpit Models layout, provider-only colors, neutral monospace IDs, non-clipping focus-visible states, visually hidden chart-table utilities, selected-model focus treatment, five-tab/table overflow, narrow-pane behavior, and reduced-motion removal of underline/chart-overlay motion in `src/styles/index.css`
- [X] T026 [P] [US1] Make browser invoke handlers argument-aware and add dynamic model analytics/history responses with root-counted pending status and structured failures for range/provider/selection, bracketing activity outside an empty selected interval, equal IDs across providers, unseen IDs, and all-unattributed coverage in `src/mocks/ipcFixtures.ts`
- [X] T027 [US1] Execute and record Claude and Codex US1 fixtures including malformed JSON between valid records, unsupported shapes, missing timestamps, invalid token siblings, scalar-limit IDs, equal cross-provider IDs, unseen IDs, and events bracketing an empty selected interval; verify timestamp-exact filter-empty scope, 100% accepted-ID representation, raw preservation, coverage, every sort, keyboard navigation, chart-table access, independent aggregate/history Retry, snapshot independence, and fixed-window five-second visibility in `specs/011-session-model-analytics/quickstart.md`

**Checkpoint**: Prospective Claude/Codex evidence produces useful Models totals
and history without a completed historical backfill or any hard-coded model list.

---

## Phase 4: User Story 2 — Understand Model History and Coverage (Priority: P2)

**Goal**: Recover all locally retained model evidence through a visible,
resumable, failure-tolerant history pass while keeping partial results usable.

**Independent Test**: Backfill retained transcripts containing known and missing
model evidence, then confirm recovered models, complete coverage accounting,
idempotent unchanged retry, partial failure reporting, and correct empty states.

### Implementation for User Story 2

- [X] T028 [US2] Implement persisted pending/running/complete/partial/failed transition and counter mutation methods, total/completed/failed root updates, explicit inventory completeness, interrupted-run reset, retry initialization, and bounded errors—leaving the foundational status read in T004—in `src-tauri/src/storage.rs`
- [X] T029 [P] [US2] Implement the nonblocking retained-source worker that persists root outcomes before source totals, assigns stable root ownership, yields between resumable batches, skips unchanged/unchanged-suppressed sources, atomically replaces changed sources, retains unreadable last-good rows, prunes only within each completed root key using explicit child-before-parent deletes, commits progress, and resolves terminal status independently from inventory completeness in `src-tauri/src/model_usage.rs`
- [X] T030 [US2] Schedule migration-pending/interrupted backfill after storage initialization and register idempotent `retry_model_history_backfill` with the shared structured error envelope without blocking window startup in `src-tauri/src/lib.rs`
- [X] T031 [US2] Extend session/project/host deletion transactions to explicitly delete matching observations and suppress retained source rows, match root analytics sessions, update model `cwd` during rename, preserve suppression until changed replacement commits, emit refresh after wrapper commits, and add neither a model TTL nor token-hourly cleanup coupling in `src-tauri/src/storage.rs` and `src-tauri/src/lib.rs`
- [X] T032 [P] [US2] Build compact pending/running/complete/partial/failed status with completed/failed root counts distinct from processed/failed/remaining source counts, retry control, incomplete labels, and live-region announcements in `src/components/analytics/models/ModelBackfillStatus.tsx`
- [X] T033 [US2] Add backfill status/retry handling that remains independent from aggregate/history Retry state, retains recovered results, and lets pending/running/partial/failed status coexist with request-local refresh errors in `src/hooks/useModelAnalytics.ts`
- [X] T034 [US2] Integrate `ModelBackfillStatus`, persisted inventory completeness, trustworthy-final scope, suppressed-source exclusion, and exact empty precedence—global no sessions, provider/range mismatch, then no reliable model evidence—while incomplete/failure states show provisional wording without replacing recovered data in `src/components/analytics/ModelsTab.tsx`
- [X] T035 [US2] Add severity-only backfill status, retry, coexisting error, and empty-state styling that remains legible at both analytics pane densities in `src/styles/index.css`
- [X] T036 [P] [US2] Add browser-demo pending/running/complete/partial/failed responses with root-complete versus root-incomplete partial states, unreadable sources, suppressed-source exclusion, filter-empty/no-session/no-model-evidence scope, and structured failures in `src/mocks/ipcFixtures.ts`
- [X] T037 [US2] Execute and record the backfill walkthrough for interruption/resume, unchanged retry, failed roots versus unreadable sources, append/rewrite/remove, sibling-source isolation, deletion non-resurrection and scope exclusion, changed-fingerprint recovery only after commit, backfill Retry coexisting with aggregate/history errors, range consistency, and existing-total invariance in `specs/011-session-model-analytics/quickstart.md`

**Checkpoint**: All recoverable retained history is visible, attribution gaps are
explicit, and failures/retries cannot duplicate, erase, or resurrect valid data.

---

## Phase 5: User Story 3 — Investigate Multi-Model Sessions (Priority: P3)

**Goal**: Let users page matching sessions, identify the deterministic primary
model, and inspect real model changes independently within every chain.

**Independent Test**: Process a parent chain that changes models plus an
interleaved subagent on another model; confirm one real parent switch, no
parent/subagent switch, deterministic primary selection, 20-row paging, and
chronological chain detail.

### Implementation for User Story 3

- [X] T038 [US3] Implement selected-model keyset session paging with 20-row default/100-row cap, opaque cursors, total/next cursor, stable recency ordering, display name, cwd, hostname, selected-model tokens/turns, last activity, distinct models, chain counts, switch flags, and range-scoped primary ranking by attributed tokens, turns, then provider/raw ID under SQLite `BINARY` collation in `src-tauri/src/storage.rs`
- [X] T039 [US3] Implement session/chain attributed and unattributed totals, distinct models, switch counts, agent IDs, and compressed model/gap segments with bounds/turn counts using turn-only adjacency, null-turn resets, token rows included in coverage but excluded from segments, repeated-model suppression, deterministic ordering, and chains ordered parent first then first activity and chain ID in `src-tauri/src/storage.rs`
- [X] T040 [US3] Implement and register `get_model_sessions` and `get_session_model_history` with shared range/provider/raw-ID validation, capped limits, opaque cursor validation, and the shared envelope using `invalid_cursor` plus stable `invalid_range`, `invalid_provider`, `invalid_model_id`, `storage_error`, and `not_found` codes in `src-tauri/src/lib.rs`
- [X] T041 [P] [US3] Create selected-model paging with reset-on-identity/range change, append-only Load more, provider/session deduplication, request-generation guards, and shared-refresh replay from page one through the prior loaded-page count with atomic replacement, retained pages on failure, and page-local Retry in `src/hooks/useModelSessions.ts`
- [X] T042 [P] [US3] Create lazy per-session chain-history loading keyed by provider/session/range; on shared refresh refetch expanded rows independently from page replay, invalidate collapsed caches, retain successful history on failure, recover stale-row `not_found`, guard generations, and expose per-row Retry in `src/hooks/useSessionModelHistory.ts`
- [X] T043 [US3] Build the initial 20-session list with native disclosure buttons using stable `aria-controls`/`aria-expanded`, provider/session identity, selected-model tokens, last activity, primary model, switch indicator, specifically named Load more/Retry controls, row-local loading/status, and chronological parent/subagent model/gap segments in `src/components/analytics/models/ModelDetailPanel.tsx`
- [X] T044 [US3] Connect table selection to model detail, pass shared refresh generation into both detail hooks, reset paging on scope changes, preserve selection while represented, clear removed selection, lazily expand session history, and reconcile refreshed/stale rows in `src/components/analytics/ModelsTab.tsx`
- [X] T045 [US3] Add compact session-row, disclosure, chain guide, model-gap, switch, pagination, focus, and narrow-pane styling in `src/styles/index.css`
- [X] T046 [P] [US3] Add argument-aware browser-demo paged sessions, deterministic primary ties, parent/subagent chains, model gaps, repeated models, and `not_found` detail responses in `src/mocks/ipcFixtures.ts`
- [X] T047 [US3] Execute and record the US3 walkthrough for parent changes, interleaved subagents, null gaps, repeated models, deterministic Unicode-scalar ties, more than 20 sessions, multi-page refresh replay, expanded-history refresh even when page replay fails, collapsed-cache invalidation, stale-row removal, independent page/row Retry, and complete chronological chain history in `specs/011-session-model-analytics/quickstart.md`

**Checkpoint**: Every multi-model session can be investigated without fabricating
switches across gaps or concurrent parent/subagent chains.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Align demos and architecture docs, audit model neutrality, and gather
the measurable completion evidence required by the specification.

- [X] T048 [P] Update schema/table inventory to version 28 and add `--codex-sessions-dir`; generate matching Claude and Codex model-bearing JSONLs with canonical root/source keys; seed observation/source/root-complete state consistent with those files including dynamic/unattributed/chain cases plus 1,001 IDs in one session; wire isolated Codex paths through `scripts/populate_dummy_data.py`, `scripts/run_quill_demo.sh`, `scripts/run_quill_demo.ps1`, `specs/001-marketing-site/contracts/seeder-cli.md`, and `specs/001-marketing-site/contracts/launcher-cli.md` without a model catalog
- [X] T049 Audit and remove identifier-specific branches, allowlists, enums, aliases, family parsing, pricing registries, lowercasing, or “current model” fallbacks from `src-tauri/src/model_usage.rs`, `src-tauri/src/storage.rs`, `src-tauri/src/models.rs`, `src/types.ts`, `src/components/analytics/ModelsTab.tsx`, `src/components/analytics/models/ModelUsageHistory.tsx`, `src/components/analytics/models/ModelUsageTable.tsx`, and `src/components/analytics/models/ModelDetailPanel.tsx`
- [X] T050 After T049 stabilizes implementation symbols, verify Models fifth-tab and selection semantics in `DESIGN.md` and `.impeccable/design.json`, then document migration 28, Claude/Codex seeder/launcher JSONL coherence, source reconciliation, attribution coverage, refresh flow, suppression, and chain semantics with validated links in `lat.md/backend.md`, `lat.md/data-flow.md`, `lat.md/frontend.md`, `lat.md/features.md`, and `lat.md/infrastructure.md`
- [X] T051 Execute and record the full isolated regression, verify all 1,001 provider-qualified IDs in the uncapped session remain represented in the complete table/detail, complete the under-10-second highest-token-model walkthrough, and capture before/after provider/session/token invariance evidence in `specs/011-session-model-analytics/quickstart.md`
- [X] T052 Build one release artifact from one recorded commit, then record exact CPU/storage/OS/fixture metadata and SC-002/SC-004/SC-007 evidence under the fixed warm-cache protocol: fixed-window five-second live visibility, three fresh-data-directory 10,000-session backfills under five minutes, root/source failures reaching terminal state within five minutes, tab/range changes rendering within two seconds during backfill, and at least 95 of 100 measured Models openings under two seconds over 100,000 observations in `specs/011-session-model-analytics/quickstart.md`
- [ ] T053 Coordinate and record SC-009's consistent 10-user parent/subagent walkthrough and verify at least 90% identify the real within-chain switch without a false interleaving switch in `specs/011-session-model-analytics/quickstart.md`
- [ ] T054 After T048–T053, run `npm run lint`, `npm run typecheck`, and `npm run build` from `package.json`; run `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, and `cargo test --manifest-path src-tauri/Cargo.toml`; run `jq empty .impeccable/design.json`; then run `lat check` for `lat.md/backend.md`, `lat.md/data-flow.md`, `lat.md/frontend.md`, `lat.md/features.md`, and `lat.md/infrastructure.md`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: Starts immediately. T002 and T003 can run alongside T001.
- **Foundational (Phase 2)**: Depends on Setup and blocks all stories. T004, T005,
  and T006 can begin in parallel; T007/T008 follow T006; T009 follows T004/T006;
  T010 follows T005/T007/T008/T009; T011 follows T010.
- **User Story 1 (Phase 3)**: Depends on Foundational and is the MVP.
- **User Story 2 (Phase 4)**: Depends on Foundational persistence and the US1
  query/UI surface; it completes historical recovery and lifecycle behavior.
- **User Story 3 (Phase 5)**: Depends on Foundational persistence and the US1
  selection surface; safest delivery follows US2 so detail reflects final scope.
- **Polish (Phase 6)**: Depends on every desired user story. T049 follows final
  feature code; T050 follows T049 and T048 so code-reference and seeder symbols
  are stable; T054 follows T048–T053 as the final gate. T053 additionally requires
  external participant coordination.

### User Story Completion Order

```text
Setup
  → Foundational schema + validation + source reconciliation
    → US1 current model totals/history UI (MVP)
      → US2 retained-history backfill/coverage lifecycle
        → US3 paged session and chain investigation
          → Polish, documentation, performance, usability evidence
```

US2 and US3 backend work can begin after Foundational APIs stabilize, but both
integrate with US1's Models surface. Recommended merge order remains
**US1 → US2 → US3**.

### Within Each User Story

- Storage queries and lifecycle rules precede command registration.
- IPC contracts precede hooks; hooks and leaf UI components can then run in
  parallel where marked `[P]`.
- Composition and styles follow stable component props.
- Manual independent validation closes each story.

## Parallel Opportunities

### User Story 1

After T010/T011 stabilize the reconciliation API, these independent files can be
worked concurrently:

```text
T014: src-tauri/src/sessions.rs startup reconciliation
T015: src-tauri/src/server.rs source-path notify reconciliation
T017: src/hooks/useModelAnalytics.ts
T018: src/components/analytics/models/ModelSummaryStrip.tsx
T019: src/components/analytics/models/ModelUsageHistory.tsx
T020: src/components/analytics/models/ModelUsageTable.tsx
T021: src/components/analytics/TabBar.tsx
T024: src/App.tsx
T026: src/mocks/ipcFixtures.ts
```

### User Story 2

After T028 fixes persisted state transitions, run the worker and independent
frontend/demo work together; keep T030 and T031 sequential afterward:

```text
T029: src-tauri/src/model_usage.rs retained-source worker
T032: src/components/analytics/models/ModelBackfillStatus.tsx
T036: src/mocks/ipcFixtures.ts backfill states
```

### User Story 3

After T040 fixes the command contract, the two hooks can run together before
detail composition:

```text
T041: src/hooks/useModelSessions.ts
T042: src/hooks/useSessionModelHistory.ts
T046: src/mocks/ipcFixtures.ts detail responses
```

## Implementation Strategy

### MVP First

1. Complete Setup and Foundational phases.
2. Complete User Story 1 through T027.
3. Stop and run the US1 independent fixture walkthrough.
4. Demo dynamic raw model totals/history before adding historical recovery or
   session-chain detail.

### Incremental Delivery

1. **US1**: Current session evidence → model totals/history and provider coverage.
2. **US2**: Retained transcripts → resumable backfill, honest gaps, retry/deletion.
3. **US3**: Selected model → paged sessions, primary model, separated chain history.
4. **Polish**: Seed data, `lat.md`, model-neutrality audit, performance/usability
   evidence, and existing checks.

## Notes

- Raw model IDs remain opaque data. No task may add a model allowlist, model enum,
  alias map, pricing table, family parser, or current-model fallback.
- Claude assistant records may carry model and direct usage together. Codex
  `turn_context` model evidence remains tokenless; Codex token-count records stay
  unattributed unless a future source supplies an explicit linkage.
- Null-model turns break switch adjacency. Token-only observations affect coverage
  but never break or create model switches.
- Model source reconciliation is keyed by provider and source path, never only by
  session ID; parent and sibling subagent files must not erase one another.
- Backend update events emit immediately after commit; client refresh uses a fixed
  one-second window whose deadline later events cannot extend.
- The `30D` range limits queries only. Model rows follow source/session retention
  and have no independent age-based expiry.
- All five commands reject with the shared structured error envelope, never a
  plain string.
- Existing token/session aggregates remain authoritative and unchanged.
- No new automated test files are authorized by this task list.
