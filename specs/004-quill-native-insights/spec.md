# Feature Specification: Quill-Native Session Insights Stream

**Feature Branch**: `004-quill-native-insights`
**Created**: 2026-05-16
**Status**: Draft
**Input**: User description: "we want to build the Best row solution above and rely on quill local data for the analysis."

## User Scenarios & Testing *(mandatory)*

The behavioral-learning analysis combines three parallel extraction streams (tool-use observations, git history, and session insights) and then synthesizes them into rules. Today the session-insights stream is the odd one out: it shells out to an external, opaque session-analysis command and reads an on-disk artifact that command writes, then feeds the result into synthesis only as loose context. This feature replaces that with a self-reliant stream that derives its signal from Quill's own locally indexed session history and participates in the analysis as a first-class, structured stream like the other two.

### User Story 1 - Analysis no longer depends on an external session-analysis command (Priority: P1)

A user opens the Learning section and runs "Analyze". The session-insights portion of the run derives its signal entirely from Quill's own indexed session history. No external session-analysis command is invoked and no command-produced on-disk artifact is read. The run completes and the insights signal contributes to the discovered rules.

**Why this priority**: This is the core of the feature and directly removes the brittle dependency that caused analysis runs to fail when the external command's behavior, flags, or output format drifted across versions. Without this, the rest of the value cannot be delivered.

**Independent Test**: Make the external session-analysis command unavailable (remove it from the executable path), then run "Analyze" with a populated local session index. The run completes, the insights stream reports signal derived from local data, and rules are produced.

**Acceptance Scenarios**:

1. **Given** a populated local session index and the external session-analysis command unavailable, **When** the user runs Analyze, **Then** the run completes and the insights stream contributes findings without error.
2. **Given** a populated local session index, **When** the user runs Analyze, **Then** no external session-analysis command is spawned and no command-written artifact directory is read during the run.
3. **Given** the insights stream yields behavioral patterns, **When** synthesis runs, **Then** those patterns are eligible to become rules on the same footing as the other two streams' patterns.

---

### User Story 2 - Insights stream is observable, scoped, and fails loudly (Priority: P2)

A maintainer inspects a completed (or failed) run record and sees the session-insights stream's per-call inference metadata (token counts, model used, duration, cost, stop reason) alongside the other streams. When the insights stream fails, the run record names the specific cause. When the user scopes analysis to a single provider, the insights stream only considers that provider's sessions.

**Why this priority**: Consistency with the other streams is the explicit goal. Cost visibility, provider scoping, and specific error reporting are the qualities the external command could never provide, and their absence previously masked failures behind a generic aggregate message.

**Independent Test**: Run Analyze and inspect the run record — confirm a session-insights metadata entry exists. Run Analyze scoped to Codex-only and to Claude-only and confirm the insights stream only draws on in-scope sessions. Force an inference failure and confirm the run record reports the specific cause for the insights stream.

**Acceptance Scenarios**:

1. **Given** a completed run, **When** the maintainer inspects the run record, **Then** the session-insights stream has a per-call inference-metadata entry consistent in shape with the other streams.
2. **Given** a provider-scoped run (Claude-only or Codex-only), **When** the insights stream runs, **Then** it derives signal only from sessions belonging to the in-scope provider(s).
3. **Given** the insights inference call fails (rate limit, invalid structured output, or timeout), **When** the run finishes, **Then** the run record reports that specific cause for the insights stream rather than a generic "no findings produced" message.

---

### User Story 3 - Insights signal alone can produce rules (Priority: P3)

In a project with little tool-use observation data and a thin git history but a rich session history, the user runs Analyze. The session-insights stream produces behavioral patterns and the run still yields rules, instead of failing because the other two streams were empty.

**Why this priority**: This corrects a real defect surfaced during investigation: a run with substantial session-insight signal still failed with "no streams produced findings" because insights only fed synthesis as context and never counted as findings. Valuable, but lower priority than removing the external dependency and achieving stream parity.

**Independent Test**: Use a project/scope where tool-use and git streams produce no findings but session history contains clear recurring behavior. Run Analyze; confirm the run completes and produces at least one rule attributable to the insights signal.

**Acceptance Scenarios**:

1. **Given** empty tool-use and git streams but session history with clear recurring behavior, **When** the user runs Analyze, **Then** the run completes successfully and produces at least one rule.
2. **Given** all three streams are empty of signal, **When** the user runs Analyze, **Then** the run fails with a clear message and no rules are created (unchanged behavior).

---

### Edge Cases

- **Empty or absent local index**: When the local session index has no in-scope sessions or no extractable signal, the insights stream contributes zero findings and the overall run does not fail because of it — the same graceful degradation the git-history stream already exhibits when no git data is available.
- **Oversized history**: When in-scope session history exceeds the inference context budget, the system deterministically selects/compresses the data so a single invocation stays within budget rather than failing or truncating arbitrarily.
- **No behavioral signal**: Sessions exist but contain no meaningful friction/outcome/goal signal — the stream returns zero patterns without erroring.
- **Inference failure**: Rate-limit, invalid structured output, or timeout on the insights call is surfaced as a specific, actionable per-stream cause and does not silently collapse into the aggregate "no findings" message.
- **Mixed-provider scope**: A combined Claude+Codex run draws on both providers' sessions; a single-provider scope excludes the other provider's sessions entirely.
- **Stale index**: The stream uses the already-maintained index state; it does not block on or trigger a separate re-index.
- **Thin indexed content**: When an in-scope session's locally indexed content is too sparse for the semantic signal in FR-002 to be derived with confidence, that session is skipped (contributes no signal) rather than producing a low-confidence or fabricated facet; the run continues with the remaining sessions.

## Clarifications

### Session 2026-05-16

- Q: What session content may be sent to the inference subprocess (privacy / data-minimization)? → A: A compressed semantic digest **with a mandatory secret/credential redaction pass** (Option B): preserve the behavioral/semantic text but mask API keys, tokens, `.env`-style values, and recognizable credentials before the content is submitted. Prompt-injection sanitization remains separate and additional. Structural-only signal (Option C) was rejected because the rule-relevant signal is inherently semantic and Option C would violate FR-002a / regress SC-006.
- Q: What project scope do Stream C's analyzed sessions use? → A: Cross-project (Option A) — all recent sessions for the in-scope provider(s) across all projects, matching the prior `/insights` baseline and Stream A's provider-only scoping. Stream A is already cross-project (no change). Stream B stays intentionally project-scoped because git history is inherently per-repository; re-scoping Stream A or B is explicitly out of scope for this feature.
- Q: Are sub-agent / sidechain sessions selected for Stream C? → A: No (Option A) — only top-level user sessions are eligible; standalone sub-agent/sidechain transcripts are excluded from selection (their activity is represented via the parent session). Keeps the signal user-behavioral, prevents recency-cap dilution by machine-driven sessions, and preserves deterministic selection.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The session-insights stream MUST derive its input signal exclusively from Quill's own locally indexed session history and locally stored session-derived metadata. It MUST NOT invoke any external session-analysis command and MUST NOT read any on-disk artifact produced by such a command.
- **FR-002**: The session-insights stream MUST derive, per in-scope session, a rule-relevant semantic signal consisting of: the underlying goal, the task outcome, friction (categorized counts and a friction detail), the session type, a brief summary, and a primary-success / anti-pattern indicator. It MUST then extract behavioral patterns from the aggregate of that signal. The system MUST NOT be required to reproduce the prior external command's pure-analytics telemetry dimensions (per-session user-satisfaction counts, a Claude-helpfulness rating, and goal-category tallies), which are out of scope for behavioral rule discovery.
- **FR-002a**: The rule-relevant semantic signal in FR-002 MUST be a superset of the signal that reaches synthesis today (aggregated friction, outcome distribution, and brief per-session summaries), so the change cannot reduce the information available to rule discovery relative to the current behavior.
- **FR-003**: The session-insights stream MUST produce structured findings (behavioral patterns and verdicts on existing rules) in the same shape and with the same field-level meaning as the other two extraction streams, such that synthesis and all downstream consumers process them without special-casing.
- **FR-004**: Findings produced by the session-insights stream MUST contribute to a run's overall findings the same way the other streams do, including allowing a run to succeed and produce rules when the session-insights stream is the only stream that yielded signal.
- **FR-005**: The session-insights stream MUST run through the same governed inference path as the other extraction streams, inheriting the same structured-output contract, the same per-invocation hang-detector timeout, the same isolation from the user's interactive configuration, and the same error classification (a failure surfaces a specific, actionable cause).
- **FR-006**: The system MUST capture and persist the session-insights stream's per-call inference metadata (input/output token counts, cache statistics, model identifier, duration, cost, stop reason) on the corresponding run record, for both successful and failed runs, consistent with the other streams.
- **FR-007**: The session-insights stream MUST honor the run's provider scope (Claude-only, Codex-only, or combined), deriving signal only from sessions belonging to the in-scope provider(s). Selection is provider-scoped but **not** project-scoped: all in-scope-provider sessions across all projects are eligible (consistent with Stream A and with the prior cross-project baseline). This intentionally differs from Stream B, whose project scoping is inherent to git history; Stream A's and Stream B's scopes are unchanged by this feature.
- **FR-008**: When the in-scope local session history contains no extractable signal, the session-insights stream MUST contribute zero findings without failing the overall run, consistent with the git-history stream's graceful-degradation behavior.
- **FR-009**: The system MUST bound the volume of session data submitted for a single extraction invocation so it stays within the inference context budget. When the in-scope history exceeds that budget, selection MUST be deterministic and recency-biased (the most recent in-scope sessions up to a fixed cap), mirroring the prior approach's behavior of running the expensive semantic pass over a recent subset of sessions rather than the entire history, plus boundary-aware compression of each selected session's content.
- **FR-010**: The migrated stream MUST preserve the run's existing real-time progress-log fidelity (per-stream status lines streamed to the UI in their established shape) and the existing concurrency model (the three extraction streams dispatched concurrently; synthesis runs only after all three complete).
- **FR-011**: After this feature, the codebase MUST NOT retain a learning-pipeline code path that shells out to the external session-analysis command, and MUST NOT retain a disabled or unreachable external-insights parsing path that could silently mask a regression.
- **FR-012**: Before any session-derived content is included in a session-insights inference invocation, the system MUST apply a secret/credential redaction pass that masks API keys, access/refresh tokens, `.env`-style `KEY=value` assignments, and other recognizable credentials, while preserving the surrounding behavioral and semantic text. This redaction applies to every selected session's digest and is in addition to — not a replacement for — the existing prompt-injection sanitization.
- **FR-013**: Session selection MUST include only top-level user sessions. Sub-agent / sidechain transcripts MUST NOT be selected as standalone sessions, MUST NOT consume recency-cap slots, and their activity is represented only via their parent session's eligibility.

### Key Entities

- **Session Insight Signal**: The per-session rule-relevant semantic signal derived from Quill's locally indexed session history — underlying goal, task outcome, friction (categorized counts + detail), session type, brief summary, and a primary-success / anti-pattern indicator — scoped to the run's provider(s) and bounded to a recency-capped subset within the inference context budget. Serves as the extraction input. Excludes pure-analytics telemetry (user-satisfaction counts, Claude-helpfulness rating, goal-category tallies) and excludes deterministic structural metadata, which Quill already owns natively. Drawn from top-level user sessions only (sub-agent/sidechain transcripts excluded, FR-013). The content underlying this signal is passed through a secret/credential redaction pass (FR-012) before extraction; redaction strips only literal secrets and is rule-neutral (behavioral patterns are preserved).
- **Insights Stream Findings**: The structured set of behavioral patterns and existing-rule verdicts produced by the session-insights stream. Identical in shape and meaning to the findings produced by the tool-use and git-history streams.
- **Learning Run Record**: The existing per-run record, extended so the session-insights stream's progress-log lines and per-call inference metadata are captured on the same footing as the other streams.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A learning analysis run completes successfully using only Quill's local data with no external session-analysis command invoked — verifiable by running with that command absent from the executable path.
- **SC-002**: 100% of learning runs are immune to absence, output-format change, or version drift of the external session-analysis command (it is no longer on any learning code path).
- **SC-003**: The session-insights stream's cost and token usage are present on the run record for 100% of completed and failed runs, where previously they were absent for all runs.
- **SC-004**: In a scenario where the tool-use and git streams produce no findings but session history contains clear recurring behavior, the run produces at least one rule (previously it failed with a generic "no findings" message).
- **SC-005**: Every session-insights stream failure reports a specific cause (rate-limited, invalid structured output, timed out, not signed in, etc.) on the run record — 0% of insights failures appear only as the generic aggregate message.
- **SC-006**: Rule-discovery quality does not regress against the correct baseline — the signal that actually reached synthesis under the prior approach (aggregated friction, outcome distribution, and brief per-session summaries over a recent subset of sessions), not the full external-command facet schema. Across a representative sample of analysis runs, the count and reviewer-judged usefulness of discovered rules is at least equivalent to that baseline; because the FR-002 signal is a superset of it (FR-002a), parity is the floor and improvement is expected.
- **SC-007**: Total end-to-end run latency remains in the same range as before — the insights stream runs concurrently with the other two and does not extend total run time beyond the slowest single stream plus synthesis.
- **SC-008**: Across an audit sample of session-insights invocations, 100% of detected secret/credential patterns (API keys, access/refresh tokens, `.env`-style assignments) in the submitted content are masked, with no measurable loss of the rule-relevant behavioral signal (SC-006 parity continues to hold).

## Assumptions

- "Quill local data" means Quill's existing locally indexed session history and the session-derived metadata already stored locally; no network call to any external session-analysis service is made for this stream.
- This feature reuses the existing local session index as-is. It does not change how sessions are indexed, nor add new indexing, nor trigger or block on a re-index; an empty index degrades gracefully (FR-008).
- The stream mirrors the established three-stream parallel dispatch and the existing model assignment for extraction work (the lightweight extraction model, not the synthesis model). No additional per-session LLM summarization pass is introduced; a single bounded extraction invocation produces the stream's findings, keeping per-run call count and cost shape comparable to today.
- The external command produced two layers: a deterministic structural-metadata layer (per-session tool counts, languages, durations, message counts, git activity, token counts, first prompt) and an LLM-derived semantic-facet layer. The structural layer is **already owned by Quill natively** (its existing index and usage data), so this feature does not reproduce it via the stream — only the rule-relevant semantic layer (FR-002) is re-derived by Quill's own extraction. Exact parity with the external command's internal heuristics is explicitly not a requirement; the baseline for non-regression is the narrower signal that actually reached synthesis (FR-002a, SC-006).
- The external command also samples which sessions get the expensive semantic pass (it analyzed a recent subset, not all history); this feature deliberately mirrors that with a deterministic recency-capped selection (FR-009) rather than analyzing every indexed session.
- The external command's human-readable HTML report and its pure-analytics telemetry dimensions (user-satisfaction counts, Claude-helpfulness rating, goal-category tallies) are explicitly out of scope and are not reproduced.
- The synthesis step and the other two streams are unchanged except that the insights stream now arrives as structured findings rather than loose context.
- The cross-stream scope asymmetry is intentional and by design: Stream A and Stream C are provider-scoped/cross-project (user-behavior signal), while Stream B is project-scoped (repository-convention signal, inherent to git). Aligning Stream A/B scopes is explicitly NOT part of this feature; if ever desired it is separate feature work with its own spec.
- No new user-facing UI for inference metadata is introduced (consistent with the prior inference-migration feature's deferral); subsequent features may surface it.
- Addressing the broader silent-failure pattern in the other streams is out of scope here; this feature only guarantees specific-cause reporting for the session-insights stream (FR-005). The wider pipeline error-surfacing change remains a separately tracked follow-up.
- This feature activates the governed inference path that the prior inference-migration feature explicitly reserved for the insights stream; it depends on that path already existing.

## Dependencies

- Requires the governed single inference path delivered by the prior inference-migration feature (003); this feature makes the session-insights stream a real consumer of it.
- Requires Quill's existing local session index to be populated to yield signal; an unpopulated index is handled by graceful degradation (FR-008), not treated as a failure.
- Replacement quality depends on Quill's indexed session **content** (not just structural metadata) being rich enough for an LLM to re-derive the FR-002 per-session semantic signal at quality comparable to a full-transcript analysis. Sessions whose indexed content is too thin are skipped (Edge Cases), and overall non-regression is validated by SC-006. If indexed content proves systematically insufficient, that is a constraint to surface during planning (it may require widening what session content is indexed — itself out of scope here).
