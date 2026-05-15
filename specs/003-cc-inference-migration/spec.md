# Feature Specification: Migrate LLM Inference to Claude Code Integration

**Feature Branch**: `003-cc-inference-migration`
**Created**: 2026-05-14
**Status**: Draft
**Input**: User description: "we need to completely replace all our anthropic api calls and use cc instead as that is the actual supported behavior. take the time to investigate and validate invoking cc will work the way we need it to"

## Background

Today, the app makes direct HTTPS calls to the Anthropic API for every LLM-driven feature (behavioral learning analysis, memory optimization, prose compression). Authentication is performed by lifting Claude Code's OAuth access token out of the user's Claude Code credential store and replaying it against `api.anthropic.com/v1/messages` with an undocumented beta header that opts the request into the Claude subscription's quota.

This works, but it is not an officially supported integration path. The credential schema, the beta header, and the OAuth-as-API-key flow are internal Claude Code implementation details that the vendor can change without notice — and has already changed once (Claude Code v2.1.52 changed the macOS Keychain service name, forcing a fallback path in our credential reader). Using a Claude subscription token outside of the Claude Code client is also a gray area with respect to the consumer subscription terms.

The supported way to consume a user's Claude subscription quota from a non-Anthropic application is to invoke the Claude Code CLI in its non-interactive ("headless") mode. The CLI exposes a stable, documented surface (`-p/--print`, `--output-format json`, `--model`, `--json-schema`, etc.) that is specifically intended for programmatic use and that Anthropic has committed to maintaining.

This feature migrates every direct LLM API call in the app to that supported surface and removes the internal-credential consumption path.

## Clarifications

### Session 2026-05-14

- Q: Should the migrated analyze pipeline preserve the current parallel dispatch of the three stream calls, or change to bounded / sequential concurrency? → A: Preserve parallel — Stream A, Stream B, and Stream C dispatched concurrently; Synthesis runs after they complete. Memory-optimization calls retain their current dispatch shape.
- Q: How should rate-limit and retry behavior work on the migrated path, and does the usage-scraping poller change? → A: No app-side retry, no rate-limit backoff, no Retry-After interpretation for inference calls — Claude Code is solely responsible for whatever it chooses to do internally during a single invocation, and any error it returns is surfaced as-is and is final for the run. The Live-Usage metadata poller is explicitly out of scope and MUST remain unchanged.
- Q: What per-call timeout should bound a single Claude Code invocation? → A: 300 seconds (5 minutes). Acts purely as a hang detector wrapping one one-shot invocation; comfortably accommodates the largest realistic Sonnet response, fails the run if exceeded.
- Q: What observability data should the migrated path capture per inference call? → A: Preserve the existing stream-by-stream text-log fidelity AND additionally store Claude Code's structured per-call metadata (input/output tokens, cache-creation and cache-read tokens, model id, total duration, time-to-first-token, total cost, stop reason, permission denials) on the corresponding run record. No new UI surfacing in this feature; storage-only with future features free to expose it.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Run a learning analysis without depending on an unsupported authentication path (Priority: P1)

A user with Claude Code installed and signed in clicks "Analyze" in the learning view. The analysis completes successfully and produces the same kinds of results (new learned rules, verdicts on existing rules, observations marked analyzed) as before, but every LLM call underlying the analysis was made by Claude Code itself on the user's behalf — not by the app reaching `api.anthropic.com` with credentials it pulled out of Claude Code's credential store.

**Why this priority**: This is the entire reason for the feature. Until the analyze pipeline is migrated, the app continues to depend on an undocumented mechanism that can break at any Claude Code release. The learning pipeline is also the largest single consumer of LLM calls (3-4 calls per run), so it is where the support risk is most concentrated.

**Independent Test**: Trigger an on-demand analysis from the UI on a system with Claude Code installed and signed in. Verify the run completes, produces rules/verdicts, and that during the run no outbound HTTPS request is made by the app process directly to `api.anthropic.com`.

**Acceptance Scenarios**:

1. **Given** Claude Code is installed and signed in, and there are unanalyzed observations and recent git history, **When** the user clicks "Analyze", **Then** the run finishes successfully, the result is recorded in the run history with status `success`, and any rules produced are written to the appropriate provider-scoped rule directories — equivalent in shape and content to the pre-migration behavior.
2. **Given** Claude Code is installed and signed in, **When** the user triggers an analysis, **Then** the app process makes no outbound network connection to `api.anthropic.com/v1/messages` itself; all LLM work is performed by Claude Code subprocesses launched by the app.
3. **Given** the periodic analysis timer fires, **When** the timer runs, **Then** the same migrated path is used (no remaining direct-API code path).

---

### User Story 2 - Run memory optimization and prose compression through the supported path (Priority: P1)

A user opens the Memories panel and runs memory optimization (or the optional prose-compression pre-pass). The optimization runs and produces suggestions or rewritten content, but every LLM call is made by Claude Code on the user's behalf.

**Why this priority**: Memory optimization shares the same direct-API consumption path as the learning pipeline. Migrating only learning would leave the app still depending on the unsupported mechanism whenever a user touches memories. Both features must move together for the migration to be complete.

**Independent Test**: From the Memories panel, run optimization on a project that has memory files. Confirm suggestions appear in the UI and that no direct outbound call to `api.anthropic.com` was made by the app.

**Acceptance Scenarios**:

1. **Given** a project has at least one memory file and the user opens the Memories panel, **When** the user runs optimization, **Then** the run produces suggestions equivalent in shape to the pre-migration behavior and the suggestions appear in the UI.
2. **Given** a project has memory files, **When** the user runs the prose-compression pre-pass, **Then** the rewritten prose is produced and stored using the same flow as before, but the underlying LLM call is performed by Claude Code.

---

### User Story 3 - See a clear, actionable error when Claude Code is missing or unusable (Priority: P2)

A user attempts to run an analysis or memory optimization on a system where Claude Code is not installed, not signed in, or is too old to support the headless invocation surface the app requires. Instead of a generic failure, they see a specific message that explains the dependency, names what's wrong (not installed / not signed in / version too old), and points them at the fix.

**Why this priority**: Migrating to a CLI dependency adds a new failure mode the app did not have before — Claude Code might be missing, broken, or out of date. Without clear surfacing, this looks identical to a generic API error, which makes the app feel broken. P2 because the feature still works for the happy path even without this polish; P1 work can ship and this can follow.

**Independent Test**: On a system where the `claude` command is not on `PATH`, trigger an analyze and confirm the UI shows a specific message naming the missing dependency rather than a generic API failure.

**Acceptance Scenarios**:

1. **Given** the `claude` command is not on `PATH`, **When** the user triggers an analysis or memory optimization, **Then** the run fails with a clear message identifying Claude Code as missing and includes installation guidance.
2. **Given** the `claude` command is present but the installed version does not support the headless interface (e.g., older than the minimum supported version), **When** the user triggers an analysis, **Then** the run fails with a message naming the version mismatch and the minimum required version.
3. **Given** Claude Code is installed but the user is not signed in (or the credentials are otherwise unusable), **When** the user triggers an analysis, **Then** the failure message names the authentication problem and tells the user how to sign in.

---

### Edge Cases

- **Concurrent analyses and concurrent Claude Code use.** If the user is actively chatting with Claude Code while an analysis runs, both invocations share the same subscription quota. The new path does not improve quota contention — it preserves the pre-migration behavior of surfacing a rate-limit-style error message. (Quota separation is explicitly out of scope; see Assumptions.)
- **Claude Code subprocess hangs.** A misbehaving model invocation, network stall, or local Claude Code bug could leave a subprocess running indefinitely. Each invocation must have a bounded timeout and fail cleanly if exceeded.
- **Model returns output that does not satisfy the requested JSON Schema.** The migration must continue to produce structured outputs of the same shape (stream findings, synthesis output, optimizer suggestions). When the model returns malformed output, the per-call failure must be surfaced through the same channels as before and not silently truncate or drop the run.
- **User has custom Claude Code hooks or skills that would interfere with automated calls.** The app's invocations must be isolated from the user's interactive configuration (hooks, slash commands, project-level CLAUDE.md content) so that an analysis run is reproducible regardless of the user's local Claude Code setup.
- **Claude Code is running interactively in the background.** Spawning additional Claude Code processes for analysis must not disrupt the user's interactive session.
- **Claude Code's rate-limit / subscription session is exhausted.** The user-visible error must remain at least as informative as before the migration (e.g., still indicates "rate limit, wait and retry").
- **Live-usage display still works.** The app's periodic poll of subscription usage state (powering the Live Usage View) is separate from LLM inference and is not affected by this feature.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST perform all LLM inference for behavioral learning analysis (observation extraction, git history extraction, insights extraction, synthesis) through the Claude Code integration path rather than direct calls to the Anthropic API.
- **FR-002**: The system MUST perform all LLM inference for memory optimization (suggestion generation and the optional prose-compression pre-pass) through the Claude Code integration path rather than direct calls to the Anthropic API.
- **FR-003**: After migration, the system MUST NOT contain any code path that opens a network connection to `api.anthropic.com/v1/messages` for LLM inference. (The separate metadata endpoint that powers the Live Usage View is out of scope.)
- **FR-004**: After migration, the system MUST NOT read the Claude Code OAuth access token from the user's Claude Code credential store for the purpose of making LLM inference calls. (Reads required by the live-usage poller, if it continues to use that endpoint, are out of scope.)
- **FR-005**: The system MUST produce outputs from each migrated call that are equivalent in shape and field-level meaning to the pre-migration outputs (Stream A/B/C findings, synthesis output, memory optimizer suggestions, prose-compression output). Downstream code that consumes these outputs MUST continue to function without modification.
- **FR-006**: The system MUST isolate its invocations from the user's interactive Claude Code configuration so that an analysis run is not affected by the user's hooks, slash commands, project-level instruction files, or session history. The invocation MUST NOT persist to the user's Claude Code session history.
- **FR-007**: The system MUST select the appropriate model (the lightweight model for stream extraction and prose work, the larger model for synthesis) for each call type, matching the pre-migration model assignment.
- **FR-008**: The system MUST enforce a structured-output contract on every call that previously used schema-validated output, such that the response is either valid and conforms to the schema, or is treated as a per-call failure.
- **FR-009**: The system MUST apply a 300-second (5-minute) timeout to each inference invocation and treat an exceeded timeout as a per-call failure with a clear, actionable error. The timeout acts purely as a hang detector around a single one-shot Claude Code invocation; there is no retry on timeout (per FR-011).
- **FR-010**: When Claude Code is not installed, not on the executable path, not signed in, or installed at a version that does not support the required headless interface, the system MUST surface a clear, actionable error in the UI for analysis and memory-optimization runs. The error MUST identify which of these conditions is the cause and, where applicable, indicate the minimum supported version.
- **FR-011**: Errors returned by Claude Code (including rate-limit, subscription-quota-exhaustion, and any other failure modes) MUST be surfaced to the user as-is. The system MUST NOT implement app-side retry, sleep-based backoff, or `Retry-After` interpretation for inference calls. Claude Code is solely responsible for any retry behavior it chooses to perform internally during a single invocation; if a call fails, that failure is final for the run. The user-visible message must still allow the user to distinguish a rate-limit failure from other failures, but the system does not act on rate-limit signals automatically.
- **FR-012**: The periodic-analysis timer MUST use the migrated path, with no remaining fallback to a direct Anthropic API code path.
- **FR-013**: The system MUST remove the OAuth-to-Bearer header-swap middleware, the rate-limit retry middleware, and the rig-core Anthropic client used for inference, because no code path will require them after the migration. (Any non-inference uses of these components — if any — must be identified during planning and explicitly retained or migrated separately.)
- **FR-014**: Within a single learning-analysis run, the system MUST dispatch the three stream calls (Stream A, Stream B, Stream C) concurrently and run the Synthesis call only after all three streams have completed. Memory-optimization calls retain their pre-migration dispatch shape. This concurrency model applies whether the underlying call is implemented as a direct API request or as a Claude Code subprocess invocation.
- **FR-015**: The metadata poller that powers the Live Usage View MUST remain functionally unchanged across this migration. Its authentication path, polling cadence, error handling, and cooldown semantics MUST NOT be modified by this feature. After the migration, this poller is the only remaining consumer of the Claude Code OAuth credential in the codebase; that consumption is explicitly retained.
- **FR-016**: For every Claude Code inference invocation, the system MUST capture and persist on the corresponding run record: (a) the same stream-by-stream and run-level text-log lines that the pre-migration path emitted (`Stream A: extracted N patterns, K verdicts`, `Synthesis: prompt size X chars, calling Sonnet`, etc.), preserved verbatim in shape and meaning; and (b) Claude Code's structured per-call metadata returned in the response envelope — input token count, output token count, cache-creation tokens, cache-read tokens, model identifier used, total invocation duration, time-to-first-token, total cost (USD), stop reason, and any permission-denial entries. Both (a) and (b) MUST be persisted for both successful and failed runs. This feature MUST NOT introduce new UI surfacing for the structured metadata in (b); subsequent features may expose it.

### Key Entities

- **Inference Call**: A single request for LLM output that the app currently issues directly to Anthropic. Each call has: a model selection (lightweight or large), a system preamble, a user prompt, a maximum output budget, and (where applicable) a JSON Schema describing the required output shape. The migration changes how the call is dispatched and how the response is read, but not these inputs.
- **Inference Failure**: A categorized failure of a single inference call. Categories that must be preserved across the migration: Claude Code missing, Claude Code version too old, not signed in, rate-limited, schema-validation failed, timed out, generic failure. Each category must be representable in the run history and in the UI.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: After this feature ships, a user running a learning analysis or memory optimization on a properly configured system has 100% of the underlying LLM work performed by Claude Code on their behalf, and 0% by direct calls from the app to Anthropic's API.
- **SC-002**: A side-by-side comparison run on the same project produces analysis results (count and shape of rules created, verdicts applied, observations analyzed; count and shape of memory suggestions) that are equivalent to the pre-migration results within the normal run-to-run variability of LLM output.
- **SC-003**: On a system where the required Claude Code CLI is not present or is not signed in, 100% of attempted analyses or memory optimizations surface a specific, named error rather than a generic failure.
- **SC-004**: There is no observable regression in the user's ability to recover from rate-limit errors: a user who encounters a rate-limit failure during the migrated path still receives a message identifying the cause and indicating that they should wait and retry.
- **SC-005**: The migration does not introduce a meaningful slowdown in the user-visible analysis duration: total wall-clock time for a typical full-mode analysis remains within roughly 1.5x of the pre-migration baseline on the same hardware and dataset.
- **SC-006**: The post-migration codebase has no remaining import or symbol references to the rig-core Anthropic client, the OAuth bearer-token credential reader (for inference purposes), or the rate-limit/header-swap middleware used by the prior direct-API path.
- **SC-007**: The post-migration codebase contains no app-side retry loop, sleep-based backoff, or `Retry-After` interpretation in any inference call path. Inference call code paths are linear: dispatch, await result, return success or final failure.
- **SC-008**: The Live Usage View continues to display live subscription usage with the same cadence, accuracy, and error-recovery behavior as before the migration.
- **SC-009**: Run records for migrated learning analyses and memory-optimization runs preserve every text-log line the pre-migration path produced AND additionally contain per-call structured metadata (tokens, model, durations, cost, cache stats, stop reason) for every Claude Code invocation made during the run.

## Assumptions

- **Claude Code is already a hard prerequisite of the app.** The current code reads Claude Code's credential file to function, and several other features (restart orchestration, plugin manager, integration manager) already assume Claude Code is installed. This migration formalizes that assumption and additionally requires the `claude` command to be invocable from the app process.
- **Quota contention is not solved by this feature.** Because Claude Code authenticates against the same Claude subscription account as the prior direct-API path, rate limits on that subscription continue to apply in the same way. This feature is about *support and ToS posture*, not about increasing throughput. Any work to separate quotas (e.g., bring-your-own API key) is a distinct future feature.
- **The live-usage poller is out of scope.** The periodic poll of the subscription usage endpoint powering the Live Usage View is a metadata call, not LLM inference. It may continue to use its current path or be migrated in a separate feature. Whichever choice is made does not block this feature.
- **The headless interface of Claude Code is treated as stable.** This feature depends on the CLI's `--print`, `--output-format json`, `--model`, `--json-schema`, `--tools`, `--no-session-persistence`, `--disable-slash-commands`, `--setting-sources`, and `--append-system-prompt` options. A live invocation against the currently installed version (`2.1.142`) was used during specification to validate that these options behave as documented: a single-shot Haiku call returned a JSON envelope containing the model's response in `result`, completed in under two seconds total, exposed no permission dialogs, and respected the supplied system prompt.
- **The lightweight and large model assignments map cleanly to Claude Code's model aliases.** The current `MODEL_HAIKU` and `MODEL_SONNET` constants correspond directly to Claude Code's `haiku` and `sonnet` aliases (or their fully qualified IDs), which the CLI accepts.
- **All structured-output calls today use schemas derivable from existing Rust types.** Each `analyze_typed<T>` call site has a corresponding `T: JsonSchema`, so the JSON Schema string required by `claude --json-schema` can be produced from the same source of truth, preserving the contract between caller and callee.
- **Per-call subprocess overhead is acceptable.** The headless `claude` invocation has measurable startup cost (observed ~140 ms in validation) compared to a direct HTTP request. For the call counts in this app (<=4 calls per analyze, <=2 per memory optimization), the added latency is within Success Criterion SC-005's tolerance.
- **The app continues to authenticate against the same Claude account.** Switching the wire protocol does not require the user to re-sign-in, re-install, or change their Claude Code configuration. If they were able to run analyses before the migration, they can run analyses after the migration with no additional setup.
