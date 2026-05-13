# Implementation Plan: Skills Breakdown Tab

**Branch**: `002-track-session-skills` | **Date**: 2026-05-12 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `specs/002-track-session-skills/spec.md`

## Summary

Add a Skills option to the existing analytics breakdown panel. Session indexing will extract reliable named skill-use events into storage, and the analytics UI will request aggregated skill counts by timeframe/all-time scope and provider filter.

The implementation follows the existing Quill pattern: Rust storage query and Tauri command, shared TypeScript model and hook support, then `BreakdownPanel` rendering inside the current Now analytics tab.

## Technical Context

**Language/Version**: Rust backend, React + TypeScript frontend
**Primary Dependencies**: Tauri v2 IPC, rusqlite storage, existing React hooks and analytics components
**Storage**: Local SQLite database managed by `src-tauri/src/storage.rs`
**Testing**: Existing repo validation commands; no new test code unless explicitly requested
**Target Platform**: Tauri desktop app
**Project Type**: Desktop analytics application with Rust backend and React frontend
**Performance Goals**: Skills breakdown query should update within one second on datasets comparable to existing analytics payloads
**Constraints**: Count only records with reliable skill identity and provider; preserve Sessions, Projects, and Hosts behavior
**Scale/Scope**: One additional analytics breakdown mode, one aggregate backend command, one stored skill-use event model

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

No enforceable project constitution has been configured; the constitution file still contains placeholder principles. Project-local instructions still apply:

- Run `lat search`/`lat expand` before work and `lat check` before completion.
- Do not commit; optional Spec Kit git commit hooks are skipped.
- Do not add test code unless explicitly requested.
- Keep changes aligned with existing Quill Rust storage, Tauri IPC, React hook, and analytics component patterns.

Gate status: PASS.

## Project Structure

### Documentation (this feature)

```text
specs/002-track-session-skills/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── skill-breakdown-command.md
└── tasks.md
```

### Source Code (repository root)

```text
src-tauri/src/
├── models.rs       # Skill usage and aggregate response structs
├── storage.rs      # migration, skill-use persistence, aggregate query
├── sessions.rs     # skill-use extraction during transcript indexing
└── lib.rs          # get_skill_breakdown Tauri command registration

src/
├── types.ts                         # Skill breakdown interfaces
├── hooks/useBreakdownData.ts        # Skills mode fetch support
└── components/analytics/
    └── BreakdownPanel.tsx           # Skills tab controls and rows

lat.md/
├── backend.md
├── features.md
└── frontend.md
```

**Structure Decision**: Use the existing analytics breakdown path instead of creating a new window or analytics tab. Skill usage is stored during the same session indexing flow that already writes `tool_actions`, then read through the same Tauri invoke pattern used by host/project/session breakdowns.

## Complexity Tracking

No constitution violations or extra complexity exceptions.
