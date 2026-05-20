# Implementation Plan: Learning System Hardening

**Branch**: `005-learning-system-hardening` | **Date**: 2026-05-17 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/005-learning-system-hardening/spec.md`

## Summary

Make the behavioral-learning loop **safe, reviewable, evidence-grounded,
reversible, and measurable** by implementing the audit's prioritized roadmap.
The work replaces the current "extracting model self-rates ≥0.95 → autonomous
`std::fs::write` to a global, all-projects rule directory" path
(`learning.rs:1497-1532`) with: (1) redaction of secrets/PII **at capture**,
applied to **every** inference input; (2) a **human review/approval queue** —
no autonomous promotion exists at all (Q1=A); (3) a provenance + version-history
+ durable-tombstone schema on `learned_rules` so every active rule is traceable,
rollback-able, and stays deleted; (4) an **evidence-weighted** promotion-
eligibility gate with observation-ID grounding and a minimum evidence cluster;
(5) a frozen-replay **evaluation harness** with a with/without regression
verdict plus learning-logic unit tests in CI; (6) an **operator accept/reject
feedback** control in the existing Learning UI as the primary outcome signal
(Q2=B); and (7) a one-time **archive-then-wipe** migration of legacy
provenance-less rules, which then only return through the new gated pipeline
(Q3=C). Net change spans `src-tauri/src/` (storage schema/migration, learning
pipeline, ingest, cc_client, rule watcher) and `src/components/learning/`
(review queue + feedback UI), and adds a CI test gate and an eval harness.

**Clarified (Session 2026-05-17)**: Q1=A autonomous promotion removed entirely
(human approval always); Q2=B per-rule operator accept/reject/bad feedback in
the Learning UI is the primary outcome signal; Q3=C legacy on-disk rules are
archived read-only then wiped and only relearned via the gated pipeline.

**Key implementation decisions (researched in `research.md`, finalized at
`/speckit-implement`)**: the redaction detector set + ordering vs. compression
(R-1), the provenance/version/tombstone schema and migration (R-2), the
review-queue + approval state machine and how it supersedes the auto-write path
(R-3), the evaluation/replay-harness design and CI gate (R-4), the operator-
feedback model and its evidence-weighting integration (R-5), evidence-weighted
gating + grounding + conflict handling (R-6), and the observability/correctness
fixes (R-7).

## Technical Context

**Language/Version**: Rust (edition 2021), `src-tauri` crate (Tauri v2 backend);
TypeScript + React 19 frontend (`src/`)
**Primary Dependencies**: tokio (async, `tokio::join!`), rusqlite (SQLite
`usage.db`, inline sequential migrations in `storage.rs`), serde + `schemars`
(typed schema for `cc_client::invoke_typed`), `notify` (rule watcher), the
internal `cc_client` module (feature 003, `claude` CLI headless inference),
`prompt_utils` (compression + `redact_secrets`), React 19 + custom IPC hooks,
Recharts
**Storage**: SQLite `usage.db` — schema currently versioned through **migration
24** (verified in `storage.rs:1761`; next is **25** — migrations 21–24 already
ship `skill_usages`/`inference_metadata`); new tables/columns for provenance,
rule versions,
suppression tombstones, evidence citations, operator feedback, evaluation
results. Filesystem `.md` rule files under `~/.claude/rules/learned/`,
`~/.config/quill/learned-rules/{codex,shared}/`, plus a new one-time legacy
archive directory.
**Testing**: `cargo test` (Rust unit/integration) — this feature **explicitly
requires** learning-logic tests + a CI gate (FR-021), overriding the general
"tests only on explicit request" policy for this surface. Frontend has no test
framework today; UI verified via quickstart manual checks.
**Target Platform**: Cross-platform desktop (Tauri: Linux/macOS/Windows)
**Project Type**: Desktop app (Rust/Tauri backend + React frontend).
**Both** backend and frontend change in this feature.
**Performance Goals**: Redaction-at-capture MUST NOT break the fast-ack ingest
contract (the `/api/v1/learning/observations` endpoint returns `202` then
inserts off-thread) — redaction runs on the background insert path, not the
request thread, and must keep per-observation overhead bounded. Analysis stays
background/periodic; the eval harness is on-demand.
**Constraints**: Fully local/offline (no remote redaction/eval service);
inference only via the `claude` CLI headless path; redaction MUST run before
persistence and before any lossy compression (FR-001/FR-003); no rule reaches a
global on-disk path without an explicit human approval action (FR-007); legacy
rules archived read-only before deletion (FR-012); the existing rule-watcher /
reconcile loop must cooperate with the new approval gate, not race it.
**Scale/Scope**: Single-user local desktop; observation volume bounded by
hooks; learning surface ≈12k LOC across `learning.rs` (1581),
`storage.rs` (6770), `cc_client.rs` (873), `git_analysis.rs` (343),
`rule_watcher.rs` (140), `prompt_utils.rs` (164), plus
`src/components/learning/`.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

`.specify/memory/constitution.md` is an **unpopulated template** — every
principle and section is a placeholder; no constitution has been ratified for
this project. There are therefore no concrete constitutional gates to evaluate.

- **Result**: PASS by vacuity (no ratified principles to violate).
- **Recommendation (out of scope here)**: ratify a real constitution via
  `/speckit-constitution`; until then this gate is a no-op for all features.
- **Self-imposed engineering gates honored**: tests + CI gate for the changed
  learning-logic surface (FR-021); deletion/replacement-biased over net-new
  subsystems; reuse of the audited feature-003 `cc_client` path; one additive
  SQLite migration; no new network surface; security-sensitive changes
  (redaction, IPC authorization, sandboxing) reviewed before merge.

No entries in **Complexity Tracking** — no violations to justify.

## Project Structure

### Documentation (this feature)

```text
specs/005-learning-system-hardening/
├── plan.md              # This file (/speckit-plan output)
├── research.md          # Phase 0 output (R-1 … R-7 decisions)
├── data-model.md        # Phase 1 output (schema + entities + state machines)
├── quickstart.md        # Phase 1 output (maintainer verification walkthrough)
├── contracts/           # Phase 1 output (internal module + IPC + HTTP contracts)
│   ├── redaction.md
│   ├── rule-governance.md
│   ├── evaluation-harness.md
│   └── ipc-and-feedback.md
├── checklists/
│   └── requirements.md  # Spec quality checklist (all items pass)
└── tasks.md             # Phase 2 output (/speckit-tasks — NOT created here)
```

### Source Code (repository root)

```text
src-tauri/src/
├── storage.rs            # +migration 25: provenance, rule_versions,
│                         #  suppression_tombstones, evidence_citations,
│                         #  operator_feedback, evaluation_results; reconcile
│                         #  cooperates with approval gate; retention race fix
├── learning.rs           # remove auto-fs::write; emit review-queue candidates;
│                         #  evidence-weighted gate; observation-ID grounding;
│                         #  min evidence cluster; IRRELEVANT/conflict handling;
│                         #  pinned synthesis model; degraded run status
├── redaction.rs          # NEW: shared detector set (creds + entropy + PII),
│                         #  applied at capture and on every inference input
├── prompt_utils.rs       # redaction moved/extended; ordering before compress
├── server.rs             # observation ingest → redact on background insert
├── cc_client.rs          # OS-level sandbox hardening for spawned CLI (H-5)
├── rule_watcher.rs       # sanitize reconcile-ingested content (H-3)
├── eval_harness.rs       # NEW: frozen replay set + with/without verdict (C-4)
└── lib.rs                # IPC: authorized approve/reject/rollback/feedback

src/components/learning/  # review/approval queue + accept-reject feedback UI
                          #  + per-run cost/latency/status surfacing

.github/workflows/        # NEW/CHANGED: cargo test + clippy CI gate (FR-021)
lat.md/                   # sync features/backend/data-flow/frontend sections
```

**Structure Decision**: Single existing desktop-app codebase; no new crate or
project. The feature is a **replacement-and-extension** of the existing learning
subsystem plus a focused frontend addition, one additive SQLite migration, one
new backend module each for redaction and evaluation, and a CI gate. No
architectural split is introduced.

## Complexity Tracking

> No Constitution Check violations (constitution unratified). Table intentionally
> empty.

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| — | — | — |
