# Implementation Plan: Migrate LLM Inference to Claude Code Integration

**Branch**: `003-cc-inference-migration` | **Date**: 2026-05-14 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/003-cc-inference-migration/spec.md`

## Summary

Replace every direct call to the Anthropic API in the inference code paths with an invocation of the `claude` CLI in headless one-shot mode. Wire each call site to a single new `cc_client` module that spawns `claude -p --output-format json` with the appropriate model, system prompt, JSON Schema, and isolation flags, captures the structured JSON envelope, and returns either the typed result or a categorized failure. Remove the rig-core Anthropic client, the OAuth-to-Bearer header-swap middleware, and the rate-limit retry middleware. Leave the live-usage metadata poller (`fetcher.rs`) untouched; it becomes the codebase's only remaining consumer of the Claude Code OAuth credential. Persist Claude Code's per-call structured metadata (tokens, model id, durations, cost, cache stats) alongside the existing text logs on every run record, with no new UI surfacing in this feature.

## Technical Context

**Language/Version**: Rust (Tauri backend, `src-tauri/`), TypeScript + React (frontend, `src/`) — no language additions
**Primary Dependencies (added)**: `tokio::process` (in-tree, no new crate), `schemars` (already a workspace dep) — used to derive JSON Schema strings from existing `JsonSchema`-deriving types
**Primary Dependencies (removed)**: `rig-core` (Anthropic client), the bespoke `reqwest-middleware` chain wrapping it (`AnthropicRateLimitMiddleware`, `OAuthHeaderMiddleware`)
**Storage**: SQLite (existing). Schema additions: structured per-call metadata persisted on the existing `learning_runs.logs` channel as a JSON-encoded supplement, OR as a new TEXT column. Decision deferred to research.md.
**Testing**: `cargo test` (Rust unit), existing manual integration via the analyze UI; no test-framework change
**Target Platform**: Desktop app (Tauri) on macOS, Linux, Windows — same as today
**Project Type**: Existing desktop app with Rust backend + React frontend; the change is backend-only
**Performance Goals**: Total wall-clock for a typical full-mode analysis within 1.5× of the pre-migration baseline (SC-005). Per-call hang detector 300s (FR-009).
**Constraints**:
- No app-side retry, backoff, or `Retry-After` interpretation in any inference path (FR-011, SC-007)
- Three stream calls dispatched concurrently; Synthesis runs after (FR-014)
- Subprocess isolated from user's interactive Claude Code configuration: hooks, slash commands, session persistence, project-level instruction files (FR-006)
- The Live Usage View's poller is untouched (FR-015, SC-008)
**Scale/Scope**: Single-user desktop. ≤4 inference calls per analyze run, ≤2 per memory optimization, ≤1 per prose-compression pre-pass

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Project constitution at `.specify/memory/constitution.md` is the unfilled template — no project-specific principles are codified. Gate is trivially passed for this iteration. Reconciliation with any future project constitution is out of scope for this feature.

**Pre-Phase 0 check**: PASS (no principles to violate)
**Post-Phase 1 check**: PASS (no principles to violate)

## Project Structure

### Documentation (this feature)

```text
specs/003-cc-inference-migration/
├── plan.md              # This file (/speckit-plan command output)
├── spec.md              # Feature specification (/speckit-specify, /speckit-clarify output)
├── research.md          # Phase 0 output (this command) — resolves the open implementation questions
├── data-model.md        # Phase 1 output (this command) — entities and storage shape
├── contracts/
│   └── cc-client.md     # Phase 1 output (this command) — internal cc_client module contract
├── quickstart.md        # Phase 1 output (this command) — maintainer walkthrough
├── checklists/
│   └── requirements.md  # Quality checklist (created by /speckit-specify)
└── tasks.md             # Phase 2 output (NOT created by /speckit-plan; /speckit-tasks emits it)
```

### Source Code (repository root)

```text
src-tauri/
├── src/
│   ├── cc_client.rs            # NEW — Claude Code subprocess invocation + JSON envelope parsing
│   ├── ai_client.rs            # MODIFIED — strip inference functions; delete if no inference-adjacent
│   │                           #   helpers remain. The two model constants migrate to cc_client.
│   ├── learning.rs             # MODIFIED — Stream A/B/C and Synthesis call sites switch from
│   │                           #   ai_client::analyze_typed to cc_client::invoke_typed.
│   │                           #   Per-call CC metadata captured into the run's structured logs.
│   ├── memory_optimizer.rs     # MODIFIED — analyze_typed and complete_text call sites switch
│   │                           #   to cc_client equivalents. Same metadata capture.
│   ├── config.rs               # MODIFIED — read_access_token() and credential-store readers
│   │                           #   stay (fetcher still uses them) but are no longer reachable
│   │                           #   from inference call paths.
│   ├── fetcher.rs              # UNCHANGED — live-usage poller (FR-015).
│   ├── models.rs               # MODIFIED — add structured fields to LearningRunPayload /
│   │                           #   MemoryOptimizationRun for CC per-call metadata.
│   ├── storage.rs              # MODIFIED — persist the new structured fields. Lightweight
│   │                           #   SQLite migration (additive column or JSON blob).
│   └── lib.rs                  # MODIFIED — register new cc_client module.
└── Cargo.toml                  # MODIFIED — remove rig-core, the OAuth/Bearer middleware crate
                                #   dependencies (if not transitively required elsewhere).

src/                            # Frontend — UNCHANGED for this feature.
lat.md/                         # Architecture knowledge graph — UPDATED for new module +
                                #   removed components + new entities.
```

**Structure Decision**: Single-module backend addition (`cc_client.rs`) replacing the inference functions of an existing module (`ai_client.rs`). The frontend is not touched. The structure deliberately keeps the call-site code (`learning.rs`, `memory_optimizer.rs`) free of subprocess-spawning concerns by routing every call through `cc_client::invoke_typed<T>` and `cc_client::invoke_text` — a one-for-one drop-in replacement for the two `ai_client` functions actually in use today.

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No violations. Constitution Check trivially passes (template, not project-specific).

---

## Phase 0 Output

See `research.md`. Resolves: subprocess wiring shape, schema generation, prompt delivery (stdin vs argv), error categorization, environment isolation, cancellation, minimum CC version handling, and metadata persistence layout.

## Phase 1 Output

- `data-model.md` — entities touched and added; the structured fields persisted per inference call.
- `contracts/cc-client.md` — the internal contract of the new `cc_client` module (function signatures, inputs, outputs, error categories).
- `quickstart.md` — maintainer walkthrough: how to verify the migration end-to-end and how to detect regressions.

## Notes for /speckit-tasks

The downstream `/speckit-tasks` command should generate the dependency-ordered task list from the artifacts in this directory. The natural ordering is:

1. Schema additions (`models.rs`, `storage.rs` migration) — independent prerequisite.
2. `cc_client.rs` skeleton + tests — independent.
3. Replace call sites one at a time, in increasing risk order: prose compression → memory optimizer typed call → Stream A → Stream B → Stream C → Synthesis.
4. Strip `ai_client.rs` inference functions; remove rig-core from `Cargo.toml`.
5. Update `lat.md/`. Run `lat check`. Commit.
