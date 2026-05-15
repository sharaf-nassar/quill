---

description: "Task list for Feature 003 — Migrate LLM Inference to Claude Code Integration"
---

# Tasks: Migrate LLM Inference to Claude Code Integration

**Input**: Design documents from `/specs/003-cc-inference-migration/`
**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/cc-client.md`, `quickstart.md`

**Tests**: Not explicitly requested in the spec or by the user. Test tasks are omitted. The `quickstart.md` defines 9 manual verifications that gate completion of the relevant user stories.

**Organization**: Tasks are grouped by user story so US1 and US2 can be implemented in parallel by different contributors once the Foundational phase is complete.

## Format: `[ID] [P?] [Story?] Description`

- **[P]**: Task can run in parallel with other [P] tasks in the same phase (different files, no incomplete dependencies).
- **[Story]**: User story tag (`[US1]`, `[US2]`, `[US3]`) — required on Phase 3/4/5 tasks, absent on Setup/Foundational/Polish.

## Path Conventions

This is the existing Quill desktop app. Rust backend lives in `src-tauri/src/`; React frontend in `src/`; architecture docs in `lat.md/`.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Wire the new module into the existing crate so it's reachable from the call-site files.

- [ ] T001 Create empty module file `src-tauri/src/cc_client.rs` and add `pub mod cc_client;` to `src-tauri/src/lib.rs` so subsequent foundational tasks can populate it incrementally
- [ ] T002 [P] Run `cargo check -p quill` in `src-tauri/` to baseline the build before any rig-core removal; record the current `rig-core` and `reqwest-middleware` versions so they can be cleanly removed in Phase 6

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Add the storage column, the new types, and the `cc_client` invocation surface. Both US1 and US2 depend on every task in this phase. Schema migration must complete before any persistence task.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [ ] T003 Add an additive SQLite migration to `src-tauri/src/storage.rs` that adds `inference_metadata TEXT` (nullable) to both `learning_runs` and the memory-optimization-run table (verify the exact table name during this task per `data-model.md`); bump the migration version following the existing migration pattern in the file
- [ ] T004 Extend `LearningRunPayload` in `src-tauri/src/models.rs` with `inference_metadata: Option<String>` (JSON-encoded) and add an equivalent field on the memory-optimization-run payload struct; preserve serde rename/skip patterns from the surrounding fields
- [ ] T005 Update the `learning_runs` and memory-optimization run persistence paths in `src-tauri/src/storage.rs` to read/write the new `inference_metadata` column from/to the corresponding model fields
- [ ] T006 [P] In `src-tauri/src/cc_client.rs`, add the foundational types from `contracts/cc-client.md`: `InvokeArgs`, `Model` enum (`Haiku`, `Sonnet`), `Phase` enum (six variants), `InvokeOutcome<T>`, `InferenceCallMetadata`, and `InferenceError` (non-exhaustive, eight variants) with `Display + std::error::Error` impls producing the user-facing messages described in `research.md` R-7
- [ ] T007 [P] In `src-tauri/src/cc_client.rs`, implement the one-shot `which::which("claude")` lookup behind a `tokio::sync::OnceCell<PathBuf>` per `research.md` R-12; return `InferenceError::ClaudeCodeMissing` on lookup failure
- [ ] T008 [P] In `src-tauri/src/cc_client.rs`, implement the private `build_command(args)` helper that assembles the full argv per `research.md` R-5 (`-p`, `--output-format json`, `--model`, `--append-system-prompt`, `--tools ""`, `--disable-slash-commands`, `--no-session-persistence`, `--setting-sources ""`, `--exclude-dynamic-system-prompt-sections`, and conditionally `--json-schema <schema>`), sets `current_dir` to the app's state directory (R-14), and scrubs `CLAUDE_CODE_*`/`ANTHROPIC_*`/`NODE_OPTIONS` from the inherited env (R-6); enable `kill_on_drop(true)`
- [ ] T009 [P] In `src-tauri/src/cc_client.rs`, implement the private `parse_envelope` helper that deserializes stdout into the envelope shape defined in `research.md` R-9 (forward-compatible, `#[serde(default)]` on numerics, no `deny_unknown_fields`); convert it to an `InferenceCallMetadata`
- [ ] T010 [P] In `src-tauri/src/cc_client.rs`, implement the private `classify_error` helper that maps `(ExitStatus, stderr, parsed-envelope-or-none, prior-IO-error)` to the appropriate `InferenceError` variant per the table in `contracts/cc-client.md` § "Error mapping"
- [ ] T011 In `src-tauri/src/cc_client.rs`, implement the public `invoke_typed<T>(args) -> Result<InvokeOutcome<T>, InferenceError>` function that: writes `args.prompt` to the child's stdin (closing on EOF — see `research.md` R-2), wraps the wait in `tokio::time::timeout(Duration::from_secs(300), ...)` (FR-009), runs the prompt through `claude --json-schema $(schemars::schema_for!(T) | serde_json::to_string)`, deserializes `envelope.result` into `T`, and returns `InvokeOutcome { value, metadata }`. `T: DeserializeOwned + schemars::JsonSchema + Send + Sync + 'static`
- [ ] T012 In `src-tauri/src/cc_client.rs`, implement `invoke_text(args) -> Result<InvokeOutcome<String>, InferenceError>` as a thin wrapper around the same plumbing as T011 but without the `--json-schema` flag, returning `envelope.result` as `String`
- [ ] T013 [P] In `src-tauri/src/cc_client.rs`, add a public helper `failed_metadata(phase: Phase, err: &InferenceError) -> InferenceCallMetadata` that builds a zero-filled metadata record with `success: false` and `failure_kind` set to the variant's stable string tag, so callers can append failure metadata to the run's blob without contorting the `Result` shape (see `contracts/cc-client.md` § "Success contract")

**Checkpoint**: Foundation ready — US1 and US2 can now proceed in parallel.

---

## Phase 3: User Story 1 — Run a learning analysis without depending on an unsupported authentication path (Priority: P1) 🎯 MVP

**Goal**: Every LLM call in the learning analyze pipeline is performed by Claude Code on the user's behalf, not by a direct call from the app to `api.anthropic.com`.

**Independent Test**: Trigger an on-demand analysis from the UI on a system with `claude` installed and signed in; verify the run completes with status `success`, no outbound HTTPS connection from the Quill process goes to `api.anthropic.com/v1/messages` (only the live-usage poller's distinct endpoint is allowed), and the run record contains a populated `inference_metadata` JSON array (Verifications 1, 2, 3, 7 in `quickstart.md`).

- [ ] T014 [US1] Replace the Stream A call site at `src-tauri/src/learning.rs:288` — swap `ai_client::analyze_typed::<StreamFindings>(...)` for `cc_client::invoke_typed::<StreamFindings>(InvokeArgs { phase: Phase::StreamA, prompt, preamble, model: Model::Haiku, max_tokens: 4096 })`; preserve the surrounding `match { Ok(...) / Err(...) }` log lines verbatim so the existing `stream_log!("Stream A: ...")` text-log fidelity is untouched (FR-016 (a))
- [ ] T015 [US1] Replace the Stream B call site at `src-tauri/src/learning.rs:379` — same substitution with `Phase::StreamB`; preserve existing `stream_log!("Stream B: ...")` lines
- [ ] T016 [US1] Replace the Stream C insights call site near `src-tauri/src/learning.rs:1094` (the insights extractor invoked from the `tokio::join!` at line 776) — same substitution with `Phase::StreamC`; preserve existing log lines
- [ ] T017 [US1] Replace the Synthesis call site at `src-tauri/src/learning.rs:515` (inside `synthesize_findings`) — swap `ai_client::analyze_typed::<AnalysisOutput>(...)` for `cc_client::invoke_typed::<AnalysisOutput>(InvokeArgs { phase: Phase::Synthesis, prompt, preamble, model: Model::Sonnet, max_tokens: 8192 })`; preserve `synth_log!("Synthesis: ...")` lines; keep the existing outer `.map_err(|e| format!("Synthesis API call failed: {e}"))` wrapper because downstream code reads this exact string shape
- [ ] T018 [US1] In `src-tauri/src/learning.rs`, introduce a per-run `Vec<InferenceCallMetadata>` accumulator local to `spawn_analysis`; append each call's metadata (or `cc_client::failed_metadata(phase, &err)` on the failure branch) immediately after each replaced call site; serialize and store on the run record via the existing `storage.update_learning_run(...)` path using the new `inference_metadata` field added in T004
- [ ] T019 [US1] Verify US1 end-to-end against `quickstart.md` Verifications 1 (happy-path), 2 (metadata persisted), 3 (text logs preserved), and 7 (concurrency preserved). Document any verification deviations in this task before marking complete.

**Checkpoint**: US1 is independently shippable. The learning analyze pipeline is fully on the supported path.

---

## Phase 4: User Story 2 — Run memory optimization and prose compression through the supported path (Priority: P1)

**Goal**: Every LLM call in the memory optimizer and prose-compression pre-pass is performed by Claude Code on the user's behalf.

**Independent Test**: From the Memories panel, run optimization on a project with memory files; confirm suggestions appear in the UI with the same shape as pre-migration, no direct outbound call to `api.anthropic.com` is made by the app process, and the run record contains a populated `inference_metadata` array (Verifications 1, 2, 3 in `quickstart.md` adapted for the memory-optimization run-record table).

- [ ] T020 [US2] Replace the memory-optimizer typed call at `src-tauri/src/memory_optimizer.rs:696` — swap `ai_client::analyze_typed(&prompt, preamble, ai_client::MODEL_HAIKU, 8192)` for `cc_client::invoke_typed::<MemoryOptimizationOutput>(InvokeArgs { phase: Phase::MemoryOptimizer, prompt, preamble, model: Model::Haiku, max_tokens: 8192 })`; preserve the existing match-and-log shape around the call
- [ ] T021 [US2] Replace the prose-compression call at `src-tauri/src/memory_optimizer.rs:933` — swap `ai_client::complete_text(&prompt, &preamble, ai_client::MODEL_HAIKU, 8192)` for `cc_client::invoke_text(InvokeArgs { phase: Phase::ProseCompression, prompt, preamble, model: Model::Haiku, max_tokens: 8192 })`; preserve surrounding error handling
- [ ] T022 [US2] In `src-tauri/src/memory_optimizer.rs`, introduce a per-run `Vec<InferenceCallMetadata>` accumulator local to `run_optimization_with_run` and `run_prose_compression`; append metadata for each call site (success or failure) and persist on the memory-optimization run record via the same pattern T018 uses for `learning_runs`
- [ ] T023 [US2] Verify US2 end-to-end against `quickstart.md` Verifications 1, 2, 3 adapted for memory-optimization runs. Document any verification deviations in this task before marking complete.

**Checkpoint**: US2 is independently shippable. The memory optimizer is fully on the supported path. Combined with US1, every inference call in the app is migrated.

---

## Phase 5: User Story 3 — See a clear, actionable error when Claude Code is missing or unusable (Priority: P2)

**Goal**: Users on systems where `claude` is missing, not signed in, or too old see a specific, named error in the analyze and memory-optimization UIs rather than a generic API failure.

**Independent Test**: PATH-mask the `claude` binary, restart Quill, trigger an analyze, confirm the UI shows a specific message naming Claude Code as missing with installation guidance (Verification 4 in `quickstart.md`).

- [ ] T024 [US3] In `src-tauri/src/cc_client.rs`, audit each `InferenceError::Display` impl to ensure the message includes (a) what failed, (b) the actionable remediation, and (c) for `ClaudeCodeMissing` the install URL `https://claude.com/claude-code/install`; for `NotSignedIn` the command `claude /login`; for `ClaudeCodeTooOld` the detected version (if known) and the suggestion `claude update` or reinstall
- [ ] T025 [US3] [P] Locate the frontend run-history component that renders the `error` field from a learning run (likely under `src/components/learning/` — verify the exact file during the task). Update it to detect and special-case the `InferenceError` variant prefixes/messages from T024 with a distinct rendering: install link rendered as a clickable element for `ClaudeCodeMissing`, a "Sign in" button affordance for `NotSignedIn`, and a plain message for the remaining variants
- [ ] T026 [US3] Verify US3 end-to-end against `quickstart.md` Verification 4 (missing CC). Optionally also exercise the synthetic rate-limit and timeout scripts (Verifications 5 and 6) to confirm those failure paths render acceptably even if they are not formally part of US3.

**Checkpoint**: US3 is shippable on top of US1 + US2. All user-visible failure modes are categorized and clearly surfaced.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Tear out the now-dead direct-API code path, remove the rig-core dependency, refresh the architecture docs, and run the final verifications.

- [ ] T027 Strip the inference surface from `src-tauri/src/ai_client.rs`: delete `analyze_observations`, `analyze_typed`, `complete_text`, `build_oauth_client`, `AnthropicRateLimitMiddleware`, `OAuthHeaderMiddleware`, `retry_after_delay`, `MAX_RATE_LIMIT_RETRIES`, `MAX_RATE_LIMIT_DELAY_SECS`, `FALLBACK_RATE_LIMIT_DELAY_SECS`, `AnthropicRateLimitError`. If anything non-inference remains (model constants, helper types), retain it; otherwise delete the file entirely and remove its `pub mod ai_client;` from `src-tauri/src/lib.rs`. Move `MODEL_HAIKU` and `MODEL_SONNET` to `cc_client` if any non-inference callers still need them — otherwise delete the constants too.
- [ ] T028 [P] Remove `rig-core` and any rig-core-only sub-crates from `src-tauri/Cargo.toml` and `Cargo.lock`. Verify `reqwest-middleware` is still required by other code (run `cargo tree -i reqwest-middleware`); leave it if used elsewhere, remove it if not.
- [ ] T029 [P] Run `cargo build` and `cargo clippy --all-targets -- -D warnings` from `src-tauri/`; address any new warnings introduced by the migration. Resolve any unused-imports or dead-code findings produced by the strip in T027.
- [ ] T030 [P] Update `lat.md/backend.md`: replace the "AI Client" section with a new section describing `cc_client.rs` (subprocess invocation surface, isolation flags, error taxonomy); remove references to rig-core, OAuth-Bearer middleware, and the rate-limit retry middleware. Add a sentence to the "AI Client" successor section noting that `fetcher.rs` remains the only consumer of the Claude Code OAuth credential.
- [ ] T031 [P] Update `lat.md/data-flow.md`: revise the "Learning Analysis Pipeline" section to reflect that Streams A/B/C and Synthesis now invoke `claude` subprocesses; update the "Memory Optimization Pipeline" section likewise. Add a one-line note that the "Usage Bucket Fetching" pipeline is unchanged by this migration.
- [ ] T032 [P] Update `lat.md/features.md`: refresh the "Learning System" and "Memory Optimizer" entries to reference `cc_client.rs` instead of `ai_client.rs`.
- [ ] T033 Run `lat check` from the repository root and resolve any link or code-ref errors introduced by T030-T032.
- [ ] T034 Execute `quickstart.md` Verification 8 (Live Usage View untouched) and Verification 9 (codebase cleanliness via `rg` checks for `rig::`, `rig_core`, `AnthropicRateLimitMiddleware`, `OAuthHeaderMiddleware`, `tokio::time::sleep` inside inference paths, and `Retry-After`). All searches must return zero matches in `src-tauri/src/` outside of comments / removed-line context.
- [ ] T035 Smoke-test the full flow once more on a clean dev session: open Quill → trigger analyze → confirm success + populated `inference_metadata` blob → run memory optimization → confirm success + populated metadata → check Live Usage View renders normally. This is the final acceptance pass for SC-001 through SC-009.

---

## Dependencies & Story Completion Order

```text
Phase 1 (Setup) ─▶ Phase 2 (Foundational) ─┬─▶ Phase 3 (US1) ─┐
                                            │                   ├─▶ Phase 5 (US3) ─▶ Phase 6 (Polish)
                                            └─▶ Phase 4 (US2) ─┘
```

- **Phase 1 → Phase 2**: T002's baseline must run before any rig-core removal; T001 must complete before any task touching `cc_client.rs`.
- **Phase 2 → Phase 3 & Phase 4**: Both US1 and US2 require the storage column (T003-T005), the `cc_client` types (T006), and the public functions (T011, T012). They DO NOT require each other.
- **Phase 3 + Phase 4 → Phase 5**: US3's error UX renders messages produced by the migrated path; the categorized errors only exist once at least one of US1/US2 is exercising the migrated path. US3 can technically begin once T011/T012 are done, but practically should follow US1/US2 so the frontend has a real call to test against.
- **All previous phases → Phase 6**: Polish is gated on US1, US2, US3 being functionally complete because Phase 6 removes the only fallback to the old code path.

## Parallel Execution Opportunities

**Within Phase 2 (Foundational)**:
- T006, T007, T008, T009, T010 all touch disjoint regions of `cc_client.rs` and can be implemented in parallel. T011 and T012 must come after T006-T010.
- T013 is independent of T011/T012 and can run in parallel with them.

**Across Phase 3 and Phase 4**:
- ALL US1 tasks (T014-T019) and ALL US2 tasks (T020-T023) operate on different files (`learning.rs` vs `memory_optimizer.rs`) and can run in parallel by two contributors.

**Within Phase 6 (Polish)**:
- T028, T029, T030, T031, T032 are largely independent (different files / different concerns). T033 (`lat check`) must come after T030-T032. T034 and T035 must come after T027-T033.

## Suggested MVP Scope

**Just US1 (Phase 1 + Phase 2 + Phase 3)** delivers a complete, shippable increment that resolves the primary support-posture concern. US2 follows in the next cycle (or same cycle if parallel), then US3 polish, then Phase 6 cleanup. The OAuth-via-rig-core path can be temporarily retained in `ai_client.rs` until Phase 6 if you ship in increments.

## Total Task Count

35 tasks across 6 phases: 2 Setup + 11 Foundational + 6 US1 + 4 US2 + 3 US3 + 9 Polish.
