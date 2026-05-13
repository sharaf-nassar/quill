# Tasks: Skills Breakdown Tab

**Input**: Design documents from `specs/002-track-session-skills/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: No new test-code tasks are included because the user did not request test code.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel because it touches a different file and has no dependency on incomplete tasks.
- **[Story]**: User story label for traceability.
- Each task includes exact file paths.

## Phase 1: Setup (Shared Types)

**Purpose**: Add shared data shapes used by backend, frontend, and the contract.

- [x] T001 Add `SkillBreakdown` response struct in src-tauri/src/models.rs
- [x] T002 [P] Add `SkillBreakdown` interface and extend `BreakdownMode` in src/types.ts

---

## Phase 2: Foundational (Skill Usage Extraction)

**Purpose**: Store reliable skill-use records before any Skills breakdown UI depends on them.

**CRITICAL**: No user story work should begin until extraction and storage exist.

- [x] T003 Add `skill_usages` table migration and indexes in src-tauri/src/storage.rs
- [x] T004 Add skill usage delete/store helpers in src-tauri/src/storage.rs
- [x] T005 Add `SKILL.md` access extraction helpers in src-tauri/src/sessions.rs
- [x] T006 Wire skill usage replacement into session re-indexing in src-tauri/src/sessions.rs

**Checkpoint**: Indexed sessions can produce reliable skill-use rows.

---

## Phase 3: User Story 1 - View Skill Counts in Breakdown (Priority: P1) MVP

**Goal**: Users can select Skills in the analytics breakdown and see per-skill usage counts for the active timeframe.

**Independent Test**: Load analytics with known recognized skill usage, select Skills, and verify rows show correct timeframe-bound totals sorted highest first.

### Implementation for User Story 1

- [x] T007 [US1] Implement `get_skill_breakdown` aggregate query in src-tauri/src/storage.rs
- [x] T008 [US1] Add and register `get_skill_breakdown` Tauri command in src-tauri/src/lib.rs
- [x] T009 [US1] Fetch Skills mode data in src/hooks/useBreakdownData.ts
- [x] T010 [US1] Add Skills tab and basic skill rows in src/components/analytics/BreakdownPanel.tsx

**Checkpoint**: Skills tab works as an MVP with timeframe-bound counts.

---

## Phase 4: User Story 2 - Compare Timeframe and All-Time Skill Usage (Priority: P2)

**Goal**: Users can toggle Skills counts between the active analytics timeframe and all indexed history.

**Independent Test**: Use data with a skill outside the selected timeframe, toggle all-time mode, and confirm only the Skills counts change.

### Implementation for User Story 2

- [x] T011 [US2] Add all-time scope support to `get_skill_breakdown` in src-tauri/src/storage.rs
- [x] T012 [US2] Pass all-time state through Skills fetches in src/hooks/useBreakdownData.ts
- [x] T013 [US2] Add all-time toggle UI for Skills mode in src/components/analytics/BreakdownPanel.tsx

**Checkpoint**: Skills counts can switch between timeframe and all-time scopes.

---

## Phase 5: User Story 3 - Filter Skill Counts by Provider (Priority: P2)

**Goal**: Users can filter Skills counts to All, Codex only, or Claude Code only.

**Independent Test**: Use data containing Codex and Claude Code skill usage, switch each badge, and verify totals match the selected provider scope.

### Implementation for User Story 3

- [x] T014 [US3] Add provider filter support to `get_skill_breakdown` in src-tauri/src/storage.rs
- [x] T015 [US3] Pass provider filter through Skills fetches in src/hooks/useBreakdownData.ts
- [x] T016 [US3] Add All, Codex, and Claude Code provider badges in src/components/analytics/BreakdownPanel.tsx
- [x] T017 [US3] Render provider-scoped empty states and counts in src/components/analytics/BreakdownPanel.tsx

**Checkpoint**: Skills counts update correctly for All, Codex, and Claude Code scopes.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Update project knowledge and run verification.

- [x] T018 [P] Document Skills breakdown behavior in lat.md/features.md
- [x] T019 [P] Document backend command and storage behavior in lat.md/backend.md
- [x] T020 [P] Document frontend analytics component changes in lat.md/frontend.md
- [x] T021 Run `pnpm typecheck`
- [x] T022 Run `cargo test --manifest-path src-tauri/Cargo.toml`
- [x] T023 Run `lat check`

---

## Phase 7: User Story 4 - Drill Skills into Per-Project Counts (Priority: P2)

**Goal**: Users can expand a multi-project skill row in the Skills breakdown to see per-(project, hostname) counts within the same time scope and provider filter.

**Independent Test**: Use data with one skill used across two project roots, expand the skill row, and verify the indented sub-rows show one row per `(cwd, hostname)` with counts that sum to the parent row.

### Implementation for User Story 4

- [x] T024 [US4] Add migration 22 (cwd/hostname columns + idx_skill_usages_skill_cwd + reingest re-arm) in src-tauri/src/storage.rs
- [x] T025 [US4] Thread `cwd` through the Claude and Codex extractors and `store_skill_usages_for_messages` (hostname via `SessionIndex::local_hostname()`) in src-tauri/src/sessions.rs and src-tauri/src/storage.rs
- [x] T026 [US4] Add `project_count` to `SkillBreakdown` and `SkillProjectBreakdown` model in src-tauri/src/models.rs
- [x] T027 [US4] Extract `compute_subdir_parent_map` helper from `merge_project_subdirs` so both project and skill-project callers share it in src-tauri/src/storage.rs
- [x] T028 [US4] Implement `get_skill_project_breakdown` aggregate query with subdir merge in src-tauri/src/storage.rs
- [x] T029 [US4] Add and register `get_skill_project_breakdown` Tauri command in src-tauri/src/lib.rs
- [x] T030 [US4] Extend `delete_project_data` to also delete from `skill_usages` by cwd in src-tauri/src/storage.rs
- [x] T031 [P] [US4] Add `SkillProjectBreakdown` interface and `project_count` to `SkillBreakdown` in src/types.ts
- [x] T032 [P] [US4] Add lazy `useSkillProjects` hook keyed by `${skillName}|${requestKey}` in src/hooks/useSkillProjects.ts
- [x] T033 [US4] Render conditional skill-row chevron, lazy-fetch projects on expand, and collapse all on filter change in src/components/analytics/BreakdownPanel.tsx

**Checkpoint**: Multi-project skills expand into per-project sub-rows that respect the active time scope and provider filter.

---

## Phase 8: Documentation Sync (Cross-Cutting)

**Purpose**: Keep `lat.md/` and the spec artifacts current with shipped behavior.

- [x] T034 [P] Update lat.md/backend.md for migration 22, the `skill_usages` table layout, and the new `get_skill_project_breakdown` command
- [x] T035 [P] Update lat.md/frontend.md for the BreakdownPanel skill-expand affordance and the `useSkillProjects` hook
- [x] T036 [P] Update specs/002-track-session-skills/data-model.md with `cwd`/`hostname` fields on `SkillUsage` and the new `SkillProjectAggregate` section
- [x] T037 [P] Append the `get_skill_project_breakdown` contract to specs/002-track-session-skills/contracts/skill-breakdown-command.md
- [x] T038 Run `lat check`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies.
- **Foundational (Phase 2)**: Depends on Phase 1 and blocks all user stories.
- **US1 (Phase 3)**: Depends on Phase 2; MVP.
- **US2 (Phase 4)**: Depends on US1 command and UI plumbing.
- **US3 (Phase 5)**: Depends on US1 command and UI plumbing.
- **Polish (Phase 6)**: Depends on implemented user stories.

### User Story Dependencies

- **User Story 1 (P1)**: Can start after foundational extraction/storage.
- **User Story 2 (P2)**: Requires the Skills command and mode from US1.
- **User Story 3 (P2)**: Requires the Skills command and mode from US1; can proceed independently of US2 after US1.

### Parallel Opportunities

- T001 and T002 can run in parallel.
- T018, T019, and T020 can run in parallel after behavior is implemented.
- US2 and US3 can be implemented in either order after US1.

## Parallel Example: Setup

```text
Task: "Add `SkillBreakdown` response struct in src-tauri/src/models.rs"
Task: "Add `SkillBreakdown` interface and extend `BreakdownMode` in src/types.ts"
```

## Implementation Strategy

### MVP First

1. Complete Phase 1 and Phase 2.
2. Complete US1.
3. Validate Skills tab shows timeframe-bound counts sorted by count.

### Incremental Delivery

1. Add all-time scope with US2.
2. Add provider badges with US3.
3. Update `lat.md/` and run verification.

## Notes

- Do not commit these changes.
- Do not add test code unless explicitly requested.
- Count only recognized `SKILL.md` loads; do not infer skill usage from prose, edits, or available-skill lists.
