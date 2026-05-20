# Implementation Plan: Quill-Native Session Insights Stream

**Branch**: `004-quill-native-insights` | **Date**: 2026-05-16 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/004-quill-native-insights/spec.md`

## Summary

Replace the Stream C `claude /insights --print` subprocess + on-disk facets parsing with a self-reliant stream that (1) selects a deterministic, recency-capped, provider-scoped set of sessions from Quill's own local data, (2) assembles a bounded per-session digest from the local Tantivy index / session store, and (3) runs a single Haiku extraction through `cc_client::invoke_typed::<StreamFindings>` with `Phase::StreamC` — producing the same `StreamFindings` shape as Stream A/B. The synthesis decision is generalized so any non-empty stream (including insights) can yield rules. Net change is backend-only in `src-tauri/src/`, dominated by deletion; no new public types, no DB/schema migration, and zero `cc_client.rs` changes (`Phase::StreamC` is already wired through metadata).

**As-built note (feature 005, 2026-05-18)**: the single Stream C extraction call (and the synthesis call referenced under Performance Goals) now uses the pinned `claude-sonnet-4-6` (`Model::Sonnet46`), not Haiku — the planned Haiku assignment was superseded for a single-model pipeline with stable cost attribution (feature 005 US5 T060/H-7, L-1). The original text is preserved as the historical record.

**Clarified (Session 2026-05-16)**: content sent to inference is a semantic digest with a **mandatory secret/credential redaction pass** (FR-012); selection is **cross-project**, provider-scoped, **top-level sessions only** (sidechains excluded, FR-007/FR-013). Stream A/B scopes are unchanged (intentional asymmetry).

**Key implementation decision (deferred to `/speckit-implement`)**: the per-session *digest assembly + context-budget allocation* heuristic (FR-009) including the FR-012 redaction pattern set/masking token — the main lever on rule-discovery quality (SC-006), several valid strategies; recorded with alternatives in `research.md` (R-4).

## Technical Context

**Language/Version**: Rust (edition 2021), `src-tauri` crate (Tauri v2 backend)
**Primary Dependencies**: tokio (async + `tokio::join!`), serde + `schemars` (typed schema for `invoke_typed`), Tantivy (existing local session index), rusqlite (existing `usage.db`), the internal `cc_client` module (feature 003), `prompt_utils` (compression)
**Storage**: Read-only consumption of existing local stores — Tantivy session index + `usage.db` (`token_snapshots`, `response_times`). No new tables, no migration, no index schema change.
**Testing**: `cargo test --lib` (existing `cc_client`-style unit tests). Per project/user policy, automated test code is authored only on explicit request; spec acceptance is verified via `quickstart.md` manual verifications and the existing run-record evidence path.
**Target Platform**: Cross-platform desktop (Tauri: Linux/macOS/Windows)
**Project Type**: Desktop app (Rust/Tauri backend + React frontend). This feature is **backend-only**; no frontend changes.
**Performance Goals**: Stream C runs concurrently with Stream A/B under the existing `tokio::join!`; one Haiku call bounded by the existing 300 s hang-detector; must not extend total run latency beyond the slowest single stream + synthesis (SC-007).
**Constraints**: Fully local/offline (no external command, no network, no `~/.claude/usage-data` reads); deterministic, cross-project, top-level-only session selection (FR-007/FR-013); mandatory secret/credential redaction of submitted content (FR-012); per-invocation content bounded to the inference context budget; preserve real-time `learning-log` line shapes and parallel dispatch (FR-010).
**Scale/Scope**: Tens of recent sessions per run (recency-capped); each digested and compressed; single extraction invocation per run.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

`.specify/memory/constitution.md` is an **unpopulated template** — every principle/section is a placeholder and no constitution has been ratified for this project. There are therefore no concrete constitutional gates to evaluate.

- **Result**: PASS by vacuity (no ratified principles to violate).
- **Recommendation (out of scope here)**: ratify a real constitution via `/speckit-constitution`; until then this gate is a no-op for all features.
- **Self-imposed engineering gates honored anyway**: no new public API surface; deletion-biased; reuses the audited feature-003 inference path; no DB migration; matches existing stream architecture (lowest-complexity option).

No entries in **Complexity Tracking** — no violations to justify.

## Project Structure

### Documentation (this feature)

```text
specs/004-quill-native-insights/
├── plan.md              # This file
├── research.md          # Phase 0 — design decisions + alternatives
├── data-model.md        # Phase 1 — entities & shapes
├── quickstart.md        # Phase 1 — maintainer verification walkthrough
├── contracts/
│   └── stream-c.md      # Phase 1 — internal Stream C module contract
├── checklists/
│   └── requirements.md  # Spec quality checklist (from /speckit-specify)
└── tasks.md             # Phase 2 — created by /speckit-tasks (NOT here)
```

### Source Code (repository root)

```text
src-tauri/src/
├── learning.rs          # PRIMARY change: delete gather_insights/InsightsData/
│                        #   InsightsFacet/insight_log!; add analyze_sessions_stream
│                        #   (Stream B-shaped); generalize the synthesis-decision
│                        #   block to treat 3 streams uniformly; update
│                        #   synthesize_findings signature (insights → StreamFindings)
├── cc_client.rs         # NO logic change; only refresh the Phase::StreamC
│                        #   "reserved for future migration" doc comment (now active)
├── sessions.rs          # READ-ONLY consumer: get_context / search / SearchFilters
├── storage.rs           # READ-ONLY consumer: get_session_breakdown(...)
├── prompt_utils.rs      # READ-ONLY consumer: compress_observation / safe_truncate
└── models.rs            # NO change (reuse existing StreamFindings/StreamPattern/RuleVerdict)

lat.md/                  # Sync at implement time: data-flow (Stream C step),
                         # features (Learning System "Stream C"), backend
                         # (Claude Code Inference Client StreamC note)
```

**Structure Decision**: Single-crate backend change inside the existing `src-tauri/src/` Tauri backend. No new modules, no new directories, no frontend. `learning.rs` is the only file with behavioral edits; `cc_client.rs` gets a comment refresh; `sessions.rs`/`storage.rs`/`prompt_utils.rs` are consumed read-only via their existing internal APIs.

## Complexity Tracking

No constitutional violations and no added architectural complexity (the design removes a subprocess + adapter layer and reuses existing types/paths). Table intentionally empty.
