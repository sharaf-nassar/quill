# Implementation Plan: Session Model Analytics

**Branch**: `011-session-model-analytics` | **Date**: 2026-07-13 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/011-session-model-analytics/spec.md`

## Summary

Persist source-backed, provider-qualified model observations from retained Claude
and Codex transcripts without a model catalog or allowlist. A dedicated,
source-scoped backfill and reconciliation pipeline recovers history without
blocking Analytics or changing existing token totals. New Tauri query contracts
power an always-available Models tab with honest attribution coverage, raw model
totals, history, and chain-aware session drill-downs.

## Technical Context

**Language/Version**: Rust 2024 edition; TypeScript 5.9.3; React 19

**Primary Dependencies**: Tauri 2, rusqlite 0.31 with bundled SQLite, serde,
chrono, sha2, Recharts 3.7

**Storage**: Local SQLite for normalized model observations, source reconciliation,
and backfill state; retained Claude and Codex JSONL transcripts remain source of
truth

**Testing**: Existing Rust checks (`cargo test`, `cargo clippy`, `cargo fmt`) and
frontend lint, typecheck, and build commands plus manual fixture validation; no
new automated test code without separate user authorization

**Target Platform**: Quill Tauri desktop app on Linux, macOS, and Windows

**Project Type**: Local-first desktop application with Rust backend and React
frontend

**Performance Goals**: Publish new observations within five seconds; complete a
10,000-session clean backfill within five minutes; return summary and table data
within two seconds for at least 95% of 100 Models-tab openings over 100,000
observations, measured with the fixed four-logical-core, 8-GiB, local-SSD protocol
and timer boundaries in `spec.md`

**Constraints**: Offline-only processing; raw model IDs remain opaque and dynamic;
no inference across missing evidence; UI stays responsive during backfill; model
analytics must not alter existing provider, session, or token totals; retained
source/session lifecycle is authoritative and no model-specific TTL is added

**Scale/Scope**: All locally retained supported transcripts; four time ranges from
1 hour through 30 days; 10,000-session backfill and 100,000-observation query
targets; parent and subagent chains remain distinct

## Constitution Check

*GATE: Passed before Phase 0 research and re-checked after Phase 1 design.*

The repository constitution is still an unratified template and defines no
project-specific gates. Applicable repository policies pass:

- Model identifiers are data, never compiled enums, allowlists, aliases, or
  version catalogs.
- All collection, reconciliation, and analytics remain local to the desktop app.
- Existing token/session totals remain authoritative; model rows add attribution
  only where source evidence supports it.
- Backfill work is resumable, nonblocking, source-scoped, and failure-tolerant.
- UI follows `PRODUCT.md` and `DESIGN.md`: dense Glass Cockpit hierarchy,
  provider color only, neutral model identity, semantic tables, visible focus,
  and reduced-motion support.
- Planning adds no automated test code. The quickstart uses existing checks and
  manual validation because tests require explicit user authorization.
- No implemented behavior changes in this planning phase require a `lat.md/`
  architecture update; implementation work must update it before completion.

## Project Structure

### Documentation (this feature)

```text
specs/011-session-model-analytics/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── model-analytics-ipc.md
│   ├── model-backfill.md
│   ├── model-observation-ingest.md
│   └── model-session-detail-ipc.md
└── tasks.md                         # Created later by /speckit-tasks

DESIGN.md                            # Five-tab and model-selection semantics
.impeccable/design.json              # Machine-readable Analytics tab specimen
```

### Source Code (repository root)

```text
src-tauri/
├── claude-integration/scripts/report-tokens.sh  # Unchanged; not model source
├── codex-integration/scripts/report-tokens.sh   # Unchanged; not model source
└── src/
    ├── lib.rs                    # Module, commands, setup worker, registration
    ├── model_usage.rs            # New extraction and reconciliation domain
    ├── models.rs                 # Serializable analytics response types
    ├── server.rs                 # Live transcript notification integration
    ├── sessions.rs               # Filesystem/root discovery and path provenance
    └── storage.rs                # Migration 28, writes, queries, deletion hooks

src/
├── App.tsx                       # Keep Analytics available without live quotas
├── components/analytics/
│   ├── AnalyticsView.tsx         # Models routing independent of snapshots
│   ├── ModelsTab.tsx             # New Models composition root
│   ├── TabBar.tsx                # Always-visible Models tab
│   └── models/
│       ├── ModelBackfillStatus.tsx
│       ├── ModelDetailPanel.tsx
│       ├── ModelSummaryStrip.tsx
│       ├── ModelUsageHistory.tsx
│       └── ModelUsageTable.tsx
├── hooks/
│   ├── useModelAnalytics.ts
│   ├── useModelSessions.ts
│   └── useSessionModelHistory.ts
├── mocks/ipcFixtures.ts          # Browser-demo IPC fixtures
├── styles/index.css              # Models layout and responsive states
└── types.ts                      # Model analytics contract types

scripts/
├── populate_dummy_data.py        # Matching schema plus Claude/Codex JSONLs
├── run_quill_demo.sh             # Isolated cross-provider demo paths
└── run_quill_demo.ps1            # Windows isolated demo paths
```

**Structure Decision**: Extend Quill's existing Rust/Tauri and React analytics
layers. Keep provider adapters and reconciliation in one new Rust domain module,
SQLite ownership in `storage.rs`, and Models UI under the established Analytics
component tree. Do not modify token-report shell scripts: transcript records are
the reliable, replayable model-evidence source.

## Implementation Phases

### Phase 1: Persistence and evidence adapters

Add migration 28 with observation, source, and singleton backfill tables,
including root-completeness counters and a foundational read-only pending-status
accessor. Define strict raw-ID and token validation, stable source/record keys,
Claude assistant extraction, exact per-dimension Codex cumulative normalization,
and adapter-owned native metadata. Store explicit null-model turns and token
observations so gaps and coverage remain honest.

### Phase 2: Reconciliation and lifecycle integration

Build a process-guarded background runner that persists root-discovery
completeness, inventories every supported source, replaces one changed source per
short transaction, retains last-known-good rows on read failure, and prunes only
after complete discovery. Resolve cross-source root graphs after adapter parsing.
Start pending or interrupted work during app setup, expose retry, and integrate
the same source replacer with startup scan plus a source-keyed live queue admitted
before session-keyed search coalescing. Existing deletion transactions explicitly
delete observation children and suppress retained sources; suppressed rows never
contribute analytics scope, and no independent expiry is introduced.

### Phase 3: Aggregation and IPC

Implement indexed range/provider aggregates, deterministic primary-model and
chain-switch calculations, bounded time-series buckets, keyset session paging,
and lazy per-session history. Register four query commands and one retry command
with one serialized `{ code, message }` error envelope. The backend emits
`model-analytics-updated` only after committed changes; the client coalesces from
the first event into a fixed one-second window and advances a shared detail
refresh generation.

### Phase 4: Models analytics UI

Keep Models reachable without live token snapshots. Add range and provider
controls, a compact summary rail, neutral attributed/unattributed history,
sortable model table, selected-model session paging, lazy chain history, and
independent aggregate/history/backfill/page/chain request states with local Retry
actions. Loaded pages replay on refresh and expanded histories refetch. Preserve
all columns through horizontal scroll on narrow panes, use provider color only
for provider identity, and provide labeled pressed filters, a semantic chart data
table, and native session disclosures.

### Phase 5: Reconciliation, documentation, and validation

Connect session/project/host deletion to source suppression and observation
removal. Update browser fixtures, the dummy-data schema plus matching retained
JSONL sources, `DESIGN.md`, `.impeccable/design.json`, and `lat.md/` including
infrastructure documentation. Run existing repository checks plus the manual
scenarios in `quickstart.md`, then record the fixed-profile backfill, refresh, and
query evidence before completion.

## Requirement Coverage

| Requirement area | Planned mechanism | Design artifact |
|---|---|---|
| Dynamic raw identity and validation (FR-001–FR-004, FR-027–FR-029) | Provider-qualified opaque strings with shared validation and no catalog | `data-model.md`, ingest contract |
| Observation fidelity and coverage (FR-005–FR-010, FR-024) | Per-record isolation, exact per-dimension deltas, explicit unattributed observations, source replacement | `data-model.md`, ingest contract |
| Chains, switches, and primary model (FR-011–FR-015) | Unbounded observations, turn-only chain adjacency, null resets, Unicode-scalar deterministic aggregation | `data-model.md`, session-detail contract |
| Deletion, refresh, and reconciliation (FR-016, FR-026, FR-032) | Suppressed-source scope exclusion, lifecycle hooks, source-key queue, commit-backed event plus shared refresh generation | backfill and ingest contracts |
| Models summaries/history/table (FR-017–FR-022a) | Range/provider aggregate IPC and semantic responsive UI | analytics IPC contract |
| Model/session drill-down (FR-023–FR-023a) | Keyset paging and lazy chain history | session-detail IPC contract |
| Empty, loading, error, and backfill states (FR-025–FR-025a, FR-030–FR-033) | Persisted root completeness, independent request states, typed errors, and region-local retry | backfill and analytics contracts |

## Complexity Tracking

No constitution violations require justification. Three dedicated tables are the
minimum durable split between immutable observations, mutable source
reconciliation state, and one application-level backfill state; model catalogs
and aggregate rollup tables are intentionally omitted.
