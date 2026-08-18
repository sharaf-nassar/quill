# pi-agent-tracking-hardening

## Problem Statement

Quill's Pi tracking extension and backend can disagree about the tracking wire
shape while still reporting the integration as alive. The 2026-08-17 audit
reproduced this with a deployed extension that emitted
`lineage.kind = "agent"` and a running Quill backend that rejected the same
session start with HTTP 422 because its closed Rust enum only accepted `root`,
`linked`, and `unresolved`. Later activity and runtime requests returned 202,
but only `session_start` creates live state, so those accepted requests changed
nothing. Search notify carried the same incompatible lineage and also failed.

The mismatch is one symptom of a broader reliability problem. Protocol and
minimum-version fields are checked only after typed JSON extraction, there is no
actual TypeScript-to-Rust wire test, lifecycle occurrence time is conflated with
the immutable session origin time, running Pi state cannot recover after Quill
restart or provider reset, orphan and nested agents can disappear from the
Sessions projection, stale spool replay can resurrect closed agents, agent
telemetry is malformed or duplicated, extension loading is ambient rather than
attested, health is global and misleading, and spool ownership/backpressure can
lose evidence.

At audit time the production extension log contained 653 HTTP 422 responses,
546 HTTP 503 responses, and 231 request failures. Fifty-two Pi spool files held
about 1.38 MB. The extension suite, Sessions-agent UI suite, and focused Rust
agent-lineage test all passed, proving the current tests do not cover the
failing integration boundary.

This feature hardens the complete Pi tracking path so every root session and
subagent is represented accurately, recoverably, and compatibly. Gaps must be
explicit. Quill must not return success for discarded state, invent liveness,
hide unattached work, duplicate hook evidence, or lose durable evidence already
persisted in a supported Pi session.

## Goals

- Make extension/backend compatibility enforceable before closed-enum payload
  deserialization, with authenticated typed responses for unsupported protocol,
  reporter version, exact Quill build, event, lineage, and lifecycle-reason
  shapes.
- Add a generated cross-language contract fixture so every envelope emitted by
  `quill.ts` passes the real Rust deserializer, validator, and router; wire-shape
  changes require a coordinated protocol and extension-version decision.
- Separate immutable session origin from each lifecycle occurrence. New,
  resume, reload, revival, replacement, shutdown, live delivery, and replay
  must use deterministic ordering that cannot leave a newer process durably
  closed or resurrect an older process in `LiveTracker`.
- Recover running Pi root and agent state after Quill restart, tracking
  disable/enable, missed startup delivery, and an `unknown_session` response
  without requiring the user to restart every Pi process.
- Preserve visible work when parent evidence is absent. An unattached agent
  must remain an explicitly unresolved independent row until it can attach,
  rather than being hidden from ranking and Sessions output.
- Fold nested Pi agents to the visible root across arbitrary supported nesting
  depth while preserving direct lineage internally, and carry the supplied
  agent role/name into the observed-agent projection.
- Define and implement consistent parent aggregation for active agent count,
  runtime, activity, tokens, and turns without double-counting child rows.
- Remove the failed-request spool. For supported persisted sessions, use Pi's
  JSONL as durable source truth: native entries own messages, tools, model, and
  usage, while the extension appends compact `quill-tracking` custom entries for
  lifecycle occurrence, process instance, direct lineage, and agent label.
  HTTP push is a live accelerator; startup/notify reconciliation replays the
  persisted source idempotently after downtime or version correction.
- Make health actionable and recovery-aware. Status must distinguish healthy
  tracking, missing or unflushed session evidence, exact-version incompatibility,
  transient transport failure, reconciliation gaps, and recovered operation.
- Emit one canonical hook/runtime event for each Pi lifecycle boundary. Remove
  missing hook names, duplicate Pre/Post pairs, false root-subagent labels, and
  assistant-text evidence for tool-only turns.
- Ensure managed, npm, project, and child-process reporter copies cannot silently
  select an incompatible or feature-incomplete reporter. Child tracking
  availability must be observable when agent extension allowlists disable
  ambient discovery.
- Preserve local-only transmission, bounded Pi hot-path work, transactional
  provider assets, existing context tools/routing behavior, and explicit gaps.
- Update `lat.md` contracts and owning test specs to match the final behavior,
  then pass `lat check` and all applicable zero-warning gates.

## Non-Goals

- Replacing Claude Code or Codex transcript-based live tracking.
- Redesigning the full Sessions interface, adding a standalone agent explorer,
  or changing the Glass Cockpit visual system beyond copy/fields needed to
  render truthful Pi state.
- Adding remote Pi session sync, off-device session upload, Windows support,
  Pi subscription limits, cost display, Memory Optimizer coverage, or brevity
  injection.
- Making Quill depend on pi-subagents internals beyond a small documented child
  lineage/reporting contract. Other Pi subagent implementations remain
  unresolved unless they provide equivalent explicit proof.
- Duplicating message bodies, prompts, or tool output in Quill custom tracking
  entries; native Pi session entries remain their sole durable owner.
- Tracking `--no-session` sessions. When Pi exposes no persistent session file,
  Quill registers no tracking state, writes no fallback journal, and shows no
  session or agent row.
- Treating retained hook audit rows as verified live process state.
- Implementing runtime fixes during this specification pipeline. This run ends
  with approved, dependency-wired P0-P3 Beads ready for implementation.
- Reopening completed delivery epic `quill-bjjo`; this remediation receives a
  new focused epic while retaining that epic and
  `specs/027-pi-tracking-extension.md` as source context.

## Backlog Inputs

None. Repository searches found no open P4 issue for Pi agent tracking,
lineage, lifecycle recovery, protocol compatibility, or spool durability.
Completed epic `quill-bjjo`, completed Feature 021, Feature 022, and
`specs/027-pi-tracking-extension.md` are required historical context, not
backlog items requiring disposition.

## Target Epic

Create a new epic titled **Harden Pi agent tracking**. Do not reopen or add new
children to completed epic `quill-bjjo`.

## User Stories

### 1. Reject incompatible reporters before data loss

As a Pi user, I want Quill and its extension to negotiate a compatible tracking
contract so an upgrade or downgrade cannot silently disable agent tracking.

Acceptance criteria:

- Authentication and bounded raw-envelope checks run before event and lineage
  enum deserialization.
- Unsupported protocol, reporter version, exact Quill build, event type,
  lineage kind, or lifecycle reason returns typed JSON with a stable error code
  and compatibility detail; no bare framework 422 reaches the extension.
- Permanent compatibility responses are logged once and surfaced in integration
  health. Exact-version mismatch pauses live push without creating a retry
  spool; the persisted Pi session remains available after the matching build is
  installed.
- A successful handshake identifies the running Quill build/protocol and the
  active reporter version/capabilities.
- Managed, npm, and project copies elect only a compatible reporter; an
  incompatible first-loaded copy cannot indefinitely block a compatible copy.
- A wire-shape change cannot pass CI without the corresponding fixture,
  protocol/version disposition, server acceptance, and package checks.

### 2. Keep lifecycle ordering truthful across resume and replay

As a user who resumes, reloads, forks, or revives Pi sessions, I want Quill to
track the current process instance rather than the original transcript creation
time.

Acceptance criteria:

- Tracking distinguishes immutable session origin time from lifecycle-observed
  time and uses lifecycle time for durable start/close ordering and liveness.
- A newer resume or revival reopens a previously closed session; a stale replay
  cannot reopen it or add a phantom agent to its parent.
- Durable lifecycle acceptance and `LiveTracker` mutation are one decision. A
  durable no-op cannot still mutate live state.
- Startup, reload, resume, fork/new replacement, shutdown, crash fallback,
  live push, and persisted-session reconciliation have deterministic
  table-driven coverage.
- Stable IDs deduplicate retries of one occurrence without collapsing separate
  occurrences for the same session.

### 3. Recover running sessions after Quill disruption

As a user with long-running Pi and subagent processes, I want tracking to recover
when Quill restarts or tracking is toggled so active work does not disappear
until every process restarts.

Acceptance criteria:

- Quill startup and provider re-enable recover every still-open Pi root and
  agent for which trustworthy current evidence exists, or explicitly mark the
  evidence unresolved.
- Activity/model/usage sent for an unknown live session cannot return an
  unqualified 202 no-op. The response causes a bounded lifecycle reannounce or
  equivalent server-side recovery.
- A root missing from `LiveTracker` no longer causes its agent children to
  disappear.
- Recovery performs a bounded persisted-Pi-session inventory and stable source
  reconciliation. It consumes explicit `quill-tracking` custom entries and
  native message evidence rather than inferring lineage or process occurrences
  from paths, mtimes, filenames, or retained hook audit rows.
- Recovery converges after transient outage without an unbounded retry loop or
  repeated user action.

### 4. Show all active Pi agents under the correct root

As a developer orchestrating nested Pi agents, I want the Sessions row to show
all active work with useful labels and no hidden descendants.

Acceptance criteria:

- Every explicitly marked live Pi agent appears exactly once: attached to a
  visible root when proof resolves, otherwise as an independent unresolved row.
  A completed unresolved agent leaves the live projection instead of becoming
  retained independent history.
- Nested descendants flatten into the visible root's active-agent rail while
  direct parent relationships remain available for diagnostics/search.
- Active descendants keep every required ancestor/root rankable without cycles
  or arbitrary-depth loss.
- Agent entries use the validated agent role/name supplied by the child
  launcher when available and retain stable session/model/runtime identity.
- Parent count, runtime, activity, tokens, and turns follow one documented
  aggregation rule; hiding child rows cannot hide their work.
- Page limits are backfilled after child suppression so requesting N rows does
  not return fewer solely because attached agents occupied storage candidates.

### 5. Reconcile persisted Pi sessions after downtime

As a user running many concurrent agents during Quill downtime or maintenance,
I want Quill to recover from the Pi sessions already persisted on disk instead
of maintaining a second lossy event queue.

Acceptance criteria:

- `pi.appendEntry` writes compact `quill-tracking` custom entries for lifecycle,
  process-instance, direct-lineage, and bounded agent-label evidence; native Pi
  entries remain the only owner of messages, tools, model, usage, and content.
- Custom entries before the first assistant response follow Pi's native buffered
  flush. If the process exits before Pi creates a session file, no durable source
  exists and Quill intentionally records nothing; the documented absence is not
  a health error and creates no alternate spool.
- Quill startup and provider re-enable enumerate the configured Pi session root,
  use stable bounded reads, and reconcile changed persisted sources
  idempotently. Live notify remains the fast path, not the sole recovery path.
- Completed sessions written entirely while Quill was unavailable become
  searchable and populate lifecycle, usage, runtime, tool, skill, and lineage
  evidence from their persisted source.
- Exact-version incompatibility leaves source files untouched. Installing the
  matching Quill/reporter pair and rerunning reconciliation recovers the data;
  no future-record quarantine or failed-request spool exists.
- Session deletion and normal retention define evidence lifetime. Quill does
  not invent a separate byte/age retention policy for tracking events.
- `--no-session` sessions remain intentionally untracked and produce no disk
  artifact, live row, analytics row, search document, health error, or retry.

### 6. Trust Pi telemetry and integration health

As an operator, I want hooks, runtime rows, and health to describe what actually
worked so I can distinguish healthy tracking from partial ingestion.

Acceptance criteria:

- One tool operation produces one PreToolUse and one PostToolUse observation.
- `turn_start` never emits a payload missing `hook_event`; root agent loops are
  not labeled as subagents; `Stop` and assistant-text runtime evidence follow
  actual settled/message semantics.
- Non-2xx hook and routing telemetry responses are typed and observable rather
  than silently cancelled.
- Health cannot be marked alive solely by an accepted mutation for a session
  that was never created.
- Transient errors clear after verified recovery; persisted-source parse,
  validation, or reconciliation gaps remain visible until a later successful
  reconciliation or explicit operator acknowledgement.
- Concurrent Pi processes cannot hide a specific compatibility/session failure
  through global last-writer-wins health.

### 7. Prove the shipped composition, not isolated halves

As a maintainer, I want release tests to run the actual extension against the
actual Quill contract so same-build tests cannot stay green during a broken
mixed deployment.

Acceptance criteria:

- TypeScript emits checked-in/generated envelopes for every lifecycle and
  lineage shape; Rust deserializes and validates those exact bytes.
- A real router integration test covers authentication, compatible acceptance,
  unknown variants, exact-version/build failure, unknown-session recovery, and
  typed response bodies.
- A real pi-subagents launch proves child reporter loading/acknowledgement,
  direct and nested parent IDs, agent names, and behavior when ambient
  extensions are disabled.
- Deployment tests cover valid foreign overwrite, newer-extension/older-server,
  older-extension/newer-server, managed/npm/project coexistence, development
  identity opt-in, rollback, and mid-process drift.
- Fleet load evidence uses concurrent persisted Pi processes and records
  aggregate request rate, handler/event-loop delay, RSS, session-file growth,
  reconciliation backlog, retry rate, and recovery.
- Each behavior-changing test has one owning `lat.md` test-spec reference.

## Constraints

- Constitution principle 1: local evidence is authoritative, and unknown,
  forward-incompatible, missed, or unauthenticated data remains explicit.
- Principle 2: extend the existing Pi extension, server, storage,
  `LiveTracker`, integration manager, Sessions IPC, and React row grammar rather
  than introducing a parallel tracking service.
- Principle 3: Pi runs the extension in-process. Synchronous handler work and
  event-loop impact must remain bounded; filesystem scanning, batching, and
  replay run off hot paths with measured budgets.
- Principle 4: provider deployment, version transitions, persisted-source
  reconciliation, and rollback remain transactional, serialized where shared,
  and last-known-good preserving.
- Principle 5: expected transport, compatibility, validation, unknown-session,
  session-parse, and recovery failures are typed with enough context to act;
  unexpected failures retain their cause.
- Principle 6: formatting, lint, typecheck, Rust checks/tests, Node tests,
  package tests, build, diff checks, and existing project gates pass with zero
  warnings.
- Principle 7: automated behavior-test additions require explicit authorization
  at the clarify gate. Every authorized key test must live at the owning layer.
- Principle 8: behavior, architecture, and test changes update matching
  `lat.md` sections and one-to-one test specs; `lat check` passes.
- Principle 9: any Sessions or Integrations presentation change follows
  `PRODUCT.md`, `DESIGN.md`, accessible copy, stable numerics, responsive
  density, and existing provider colors.
- Principle 10: define and measure budgets for handler/event-loop delay,
  concurrent fleet rate, Pi session inventory/reconciliation, recovery time,
  Sessions read latency, and nested-agent overlay cost.
- Principle 11: all tracking remains local loopback, contains no message body,
  and sends nothing off-device without a separate opt-in design.
- Principle 12: implementation is Beads-tracked and ships only after the
  clarify, analyze, independent review, compatibility, recovery, and quality
  gates approve it.
- Pi support remains `>=0.84.0 <1` unless current API evidence requires a
  deliberate minimum-version change.
- Preserve the existing eight `quill_` tools and context-router behavior except
  where extension-loading separation requires a tracking-only child entry.
- The active unrelated task `quill-9swu` owns its staged retention-doc changes;
  this feature must not overwrite or absorb them.

## Open Questions

1. When an agent's parent cannot be recovered, should the independent fallback
   row remain visible indefinitely in retained Sessions history, or only while
   the agent is live/unresolved?
2. Should active and completed Pi agent tokens/turns aggregate into the root's
   displayed totals, remain separately inspectable, or support both views?
3. Is arbitrary-depth flattening into the root the intended user-visible model,
   or should the UI expose one additional nested grouping level?
4. Must Quill tracking remain available inside child processes whose launcher
   intentionally disables ambient extensions, and if so may Quill define a
   mandatory tracking-only child reporter contract with pi-subagents?
5. What compatibility window must managed and npm extension releases support
   across Quill downgrade/upgrade, and may incompatible future spool records be
   retained until the user upgrades?
6. What explicit operator action should clear a durable spool-gap warning after
   evidence is reconciled or deliberately discarded?
7. Does the user authorize the new automated cross-wire, lifecycle, spool,
   nested-agent, deployment-skew, and concurrent-fleet regression tests required
   by constitution principle 7?

## Spec Review

Six parallel review passes covered requirements, gaps, ambiguity, feasibility,
scope, and stakeholders. Cross-dimension agreement was strongest on the need to
freeze product semantics before decomposing the lifecycle, spool, health, and
reporter work. The implementation is feasible inside the existing stack, but it
is a coordinated protocol/storage redesign rather than a small patch.

### Critical Questions (answer before planning)

1. **What is the MVP boundary?** Recommended: this release must fix compatible
   ingestion, lifecycle ordering/recovery, orphan and nested live-agent
   visibility, safe spool ownership, truthful aggregate health/election, and
   the small telemetry defects. Defer completed child token/turn reaggregation,
   historical reparenting, hierarchy drilldown, spool browser/export, and the
   exhaustive deployment/fleet qualification program to phase 2. Is that cut
   approved? Flagged by: scope, feasibility, requirements.

2. **What is the visible agent model?** Recommended: flatten every supported
   descendant into the existing root rail; preserve direct lineage only in
   diagnostics/search; keep a missing-parent agent as an independent unresolved
   row under normal Sessions retention, then attach the same identity if proof
   arrives later. No additional nesting UI. Is that the intended behavior?
   Flagged by: requirements, ambiguity, scope, stakeholders.

3. **Which metrics aggregate into the root in MVP?** Recommended: preserve the
   established split. Family activity and active runtime include descendants;
   agent count/runtime remain explicit; root turn count stays root-only;
   descendant token/turn detail remains separately inspectable. Completed
   token/turn rollup into the root is phase 2 so the correctness release does
   not become an analytics migration. Approve this rule? Flagged by:
   requirements, ambiguity, feasibility, scope, stakeholders.

4. **Must supported `pi-subagents` children remain tracked when ambient
   extensions are disabled?** Recommended: yes. Define a versioned,
   tracking-only child reporter that `pi-subagents` explicitly injects and
   acknowledges, with no context tools or routing handlers. Other launchers
   remain unresolved unless they implement the same small contract. This
   explicitly supersedes Feature 027's no-community-package-dependency boundary.
   Approve? Flagged by: gaps, feasibility, scope, stakeholders.

5. **What compatibility, trust, disable, and rollback policy should ship?**
   Recommended: support current and immediately previous protocol generations;
   release server support before reporter emission; quarantine bounded future
   records; require reload/removal for pre-broker legacy copies; prefer official
   managed/npm identities, with project/development reporters opt-in; disabling
   Pi rejects ingestion from every channel without modifying user-owned
   packages. Database migrations remain forward-only and rollback outside the
   window requires restoring the pre-upgrade database or fixing forward. Is
   that risk/support window accepted? Flagged by: requirements, gaps,
   feasibility, scope, stakeholders.

6. **What evidence-loss and operator-recovery policy should govern the bounded
   spool?** Recommended: reserve capacity for lifecycle, lineage, and search
   notify; drop lower-value activity/runtime samples first and fairly when the
   hard cap is exhausted; keep future records in a separate bounded quarantine
   across disable/rollback; verified reconciliation clears transient gaps;
   corruption/drop warnings require previewed confirmation and a durable audit
   before discard. Uninstall offers an explicit keep/discard decision rather
   than silently deleting quarantine. Approve? Flagged by: requirements, gaps,
   ambiguity, feasibility, stakeholders.

7. **Do you authorize the required automated tests?** Recommended: authorize
   the smallest owning-layer cross-wire, lifecycle/recovery, spool/crash,
   nested-agent, reporter-loading, deployment-skew, migration, and concurrent
   fleet suites, each with one matching `lat.md` test-spec reference. Without
   this authorization Story 7 and constitution principle 7 cannot pass.
   Flagged by: all dimensions.

### Technical Decisions (self-resolved — veto at the gate to override)

- **Raw compatibility boundary:** accept at most a 1 MiB body as raw bytes,
  authenticate before revealing compatibility detail, inspect a shallow open
  envelope, then deserialize accepted tags into closed domain types. Return
  generic `401`; typed `400` for malformed/invalid input; typed `409` for
  `unknown_session`; typed `426` for protocol/version/capability mismatch;
  `429` with retry guidance; and typed `503` for transient unavailability.
- **Versioning:** required-field, closed-variant, or semantic incompatibility
  bumps the protocol. Backward-compatible reporter behavior/capability changes
  bump extension SemVer only. Checked-in fixtures cover the one accepted exact
  protocol/reporter/build generation plus explicit typed rejection fixtures for
  older and newer generations.
- **Lifecycle identity:** key durable sessions by provider, normalized host, and
  session id. Carry immutable `origin_at`, current `occurred_at`, random
  process-instance id, stable occurrence id, and per-instance sequence. A newer
  accepted start supersedes the prior instance; stale-instance activity/end
  cannot mutate it. Equal sequence is duplicate; lower sequence is stale.
- **Atomic disposition:** validate a whole envelope first, apply it in wire
  order inside one transaction, and return `applied`, `duplicate`, `stale`, or
  `unknown_session` outcomes. Mutate `LiveTracker` only from committed applied
  outcomes.
- **Recovery:** use a tracking-only heartbeat/reannouncement every 30 seconds,
  immediate reannounce after `unknown_session`, one replay of the original
  mutation, and a per-session cooldown. Durable open state rehydrates as
  recovering/unresolved, never active, until same-instance proof arrives. Keep
  the existing 15-minute crash backstop.
- **Lineage graph:** persist direct parent, visible root, validated agent role,
  and process instance. Resolve with memoization, a visited set, and maximum
  depth 64. Cycle, missing ancestor, cross-host conflict, and depth overflow
  become typed unresolved reasons. Suppress a child row only after a visible
  root projection exists.
- **Pagination:** one shared aggregation path repeatedly fetches candidates until
  it returns `min(requested limit, eligible visible rows)`, ordered by activity,
  provider, normalized host, and session id.
- **Persisted session source:** remove the failed-delivery spool. Append compact
  `quill-tracking` custom entries for lifecycle/process/lineage evidence and use
  native Pi entries for content, usage, model, and tool/runtime evidence.
  Startup and provider re-enable enumerate the configured Pi session root and
  reconcile changed sources through stable full-file snapshots, source
  fingerprints, event IDs, and per-source completion state. Live push/notify is
  acceleration only. A missing session file is an intentional untracked no-op.
- **Health:** store orthogonal per-reporter/process dimensions for connection,
  compatibility, lifecycle creation, transport, child-reporter acknowledgement,
  persisted-source reconciliation gap, and recovery. Aggregate the worst active
  condition plus affected count; only same-reporter/source recovery clears its
  transient error.
- **Reporter election:** new reporters register candidates before one delegating
  wrapper owns handlers. Prefer compatible managed, then npm, then opted-in
  project/development candidates; within a channel require the exact server
  protocol/reporter version pair and prefer the highest compatible capability.
  A renewable lease re-elects after failure or capability drift. Legacy
  pre-broker handlers require Pi reload/removal; exact-version mismatch remains
  typed and inert rather than entering a compatibility window.
- **Canonical telemetry:** `tool_execution_start/end` exclusively own
  PreToolUse/PostToolUse; `tool_call` remains routing-only; `turn_start` emits no
  hook without a defined mapping; explicit child lineage supplies subagent
  semantics; assistant-text evidence comes only from an assistant message that
  contains text; Stop follows settled completion. Non-2xx telemetry responses
  enter typed health.
- **Cross-wire source of truth:** deterministic TypeScript envelope builders
  generate JSONL fixtures for every event, reason, lineage, optional-field, and
  compatibility shape. Rust feeds those exact bytes through the authenticated
  router, decoder, validator, storage disposition, and response body. CI fails
  on fixture drift.
- **Performance floors:** preserve synchronous handler max `<=10 ms`, local I/O
  timeout `1500 ms`, 1,000 events/minute for ten minutes, at least 4x server
  headroom, Sessions read max `<=300 ms`, and no more than 10% p95 regression
  from pre-change baselines for event-loop delay, overlay, reconciliation, or
  RSS. Benchmark 64 concurrent persisted child launches, require zero lost
  lifecycle starts after source reconciliation, and recovery within the selected
  30-second reannounce interval plus one request timeout.
- **Migration/rollout:** add forward-only durable lifecycle-instance, direct
  lineage, reporter-health, event-receipt, and source-reconciliation metadata in
  one migration. Ship the desktop server and managed reporter as one exact
  pair; publish the matching npm reporter only after desktop availability.
  Preserve a pre-migration database backup for rollback evidence.
- **Trust boundary:** the bearer secret cannot protect against malicious code
  running as the same user. Official identity checks and project/development
  opt-in are product safety/diagnostic controls, not cryptographic attestation.
- **Sensitive metadata:** bounded launcher role/name may persist locally in the
  Pi session's custom tracking entry, but is excluded from search, routine logs,
  and support export by default. Task- or prompt-derived freeform labels are
  rejected.

### Non-Blocking Observations

- Session reconciliation may find a moved, deleted, malformed, or partially
  flushed transcript. That outcome is an explicit source/indexing gap; no
  message-body fallback or alternate journal enters scope.
- No compatibility promise can make arbitrary pre-hardening reporters
  cooperative. The exact-pair release/runbook requires reload or removal and
  server-side dedupe during the transition.
- Because supported tracking evidence lives in Pi session files, prolonged
  Quill downtime does not create a second capacity-limited queue. Evidence
  lifetime follows Pi session deletion and Quill retention policy.
- Mixed-version and recovery errors need stable remediation copy: install the
  exact Quill/reporter pair, reload Pi, reconcile persisted sessions, or restore
  the pre-migration database.
- The existing Feature 027 protocol-degradation and spool tests are superseded:
  mismatch becomes typed/inert, and persisted-session reconciliation replaces
  failed-request replay.
- Existing `lat.md` claims for typed protocol `400`, reporter coexistence,
  lifecycle continuity, spool caps, health, and explicit-agent suppression need
  a precedence rewrite in this feature rather than additive contradictory text.
- The likely critical path is 6-10 engineer-weeks after clarification, with
  external scheduling risk for the `pi-subagents` contract and deterministic
  multi-process crash/reconciliation tests.

## Clarifications

**Q1: What is the MVP boundary?**

A: **Option B.** Ship every accepted audited issue as one coordinated feature;
do not split correctness, recovery, composition, deployment-skew, and fleet
qualification into separate product phases. The metric rule in Q3 still defines
what is correct rather than creating a deferred completed-token/turn feature.

**Q2: What is the visible agent model?**

A: **Option B.** An unresolved agent is an independent row only while it is
live. Completion removes that unresolved projection instead of retaining it in
Sessions history. If direct/root proof arrives while the process is live, the
same identity attaches to the root rail.

**Q3: Which metrics aggregate into the root?**

A: **Option A.** Preserve the established split: family activity and active
runtime include descendants; agent count/runtime remain explicit; root turn
count remains root-only; descendant token/turn evidence remains separately
inspectable and is counted once in provider/project/global analytics.

**Q4: Must supported `pi-subagents` children remain tracked when ambient
extensions are disabled?**

A: **Option A.** Yes. Define a versioned tracking-only reporter that
`pi-subagents` injects and acknowledges even under `--no-extensions`, without
context tools or routing handlers. Other launchers remain unsupported/unresolved
unless they implement the same explicit contract. This supersedes Feature 027's
no-community-package-dependency boundary.

**Q5: What compatibility, trust, disable, and rollback policy ships?**

A: **Option B for compatibility.** Require an exact Quill server/protocol and
reporter version pair; do not dual-read the previous protocol. Mismatch is typed,
inert, and recoverable by installing the exact pair and reloading Pi. Official
managed/npm reporters are accepted by default; project/development reporters
require explicit opt-in. Disabling Pi rejects ingestion from all channels
without modifying user-owned packages. Forward-only schema changes preserve a
pre-upgrade database backup for rollback.

**Q6: What evidence-loss and recovery policy governs offline tracking?**

A: **Superseded by persisted-session source truth.** Do not support
`--no-session`; it is intentionally untracked and produces no fallback artifact.
For supported sessions, remove the failed-request spool entirely. Pi's native
JSONL owns messages, tools, model, usage, and runtime evidence. The extension
uses `pi.appendEntry` to persist compact lifecycle, process-instance, direct
lineage, and bounded agent-label custom entries in that same session. HTTP push
and notify accelerate live state; Quill startup/provider re-enable inventory and
reconcile persisted Pi sessions after downtime or exact-version correction.
Evidence lifetime follows Pi session retention, so no separate spool capacity,
quarantine, eviction, or discard policy exists.

Pi buffers entries until the first assistant message creates the session file.
A persistent-mode process that exits before that flush has no durable Pi session
and is explicitly untracked; Quill creates no alternate journal. This is the
same absence-of-source rule as `--no-session`.

**Q7: Are the required automated tests authorized?**

A: **Option A.** The user authorizes the smallest owning-layer cross-wire,
lifecycle/recovery, persisted-session reconciliation, nested-agent,
tracking-only child reporter, deployment-skew, migration, telemetry, and
concurrent fleet suites. Each key test receives one matching `lat.md` test-spec
reference under constitution principle 7.

## Architecture Approach

Replace Pi's failed-request spool with a source-backed design that treats a
persisted Pi session as the durable tracking record and HTTP as a live
acceleration path.

The extension supports only persistent Pi sessions. At `session_start` it checks
that the SessionManager is persistent and exposes a session path. If not, it
registers no tracking state, writes no tracking entry, emits no health error,
and leaves `--no-session` completely outside Quill. A persistent session may
still be buffered until its first assistant message; Quill accepts that a
process that exits before Pi flushes a file has no durable source and remains
untracked.

For a supported session, the extension creates one process-instance UUID shared
across extension reloads in the same Pi process. Before sending a lifecycle or
lineage mutation, it calls `pi.appendEntry("quill-tracking", data)` with the
exact event UUID and payload the HTTP live path uses. The custom entry contains
only protocol/reporter version, event/occurrence identity, process instance,
per-instance sequence, origin and occurrence timestamps, lifecycle reason,
direct lineage proof, and a bounded launcher role/name. Pi's native message,
model-change, tool-call/result, and assistant-usage entries remain the sole
durable owner of content, runtime, tool, model, and token evidence.

Quill adds Pi to startup and periodic source reconciliation. The reconciler
performs a stable bounded full-file snapshot rather than trying to tail or infer
active-tree state from rewrites. It parses the header, native entries, and
`quill-tracking` custom entries, builds one source-owned snapshot, and replaces
that session's durable lifecycle, runtime, usage, tool, skill, and lineage rows
atomically. Existing notify remains the low-latency changed-source admission
path; startup inventory and the periodic backstop recover sessions completed
while Quill was down. Content fingerprints and persisted generations avoid
reparsing unchanged files. A source that changes during its stable read retains
last-known-good rows and retries through the existing coordinator.

Live HTTP uses protocol v2 and an exact server/reporter version pair. The route
authenticates and bounds raw bytes before examining open discriminator strings,
then converts supported shapes into closed Rust domain types. The same event IDs
appear in custom entries and live pushes, so live delivery and later source
replacement cannot double-count. `unknown_session` triggers one immediate
lifecycle reannounce; a 30-second non-persisted heartbeat repairs live state
after Quill restart without adding heartbeat entries to the Pi file. Durable
open state rehydrates as recovering, not live, until same-process proof arrives.

Lineage persists direct parent identity, visible root identity, process
instance, and bounded agent role. The live graph resolves ancestors with a
visited set, memoized root lookup, and depth limit 64. A live child with no
visible parent remains an unresolved independent row; completed unresolved
children disappear. Resolved descendants flatten into the existing root rail,
while direct lineage stays available to search and diagnostics. The established
metric split remains: family activity/active runtime include descendants,
agent count/runtime remain explicit, root turns remain root-only, and child
usage/turn evidence remains separately inspectable and counted once globally.

Reporter coexistence moves from first-loaded ownership to a process-global
candidate registry with one delegating handler set. Candidates declare source,
exact reporter/protocol version, capabilities, and extension path. Exact-match
managed and npm candidates are accepted by default; project/development
candidates require opt-in. Legacy pre-broker copies require removal/reload.
The root reporter exposes its active extension path and tracking capability to
`pi-subagents`; supported child launches explicitly inject that same file under
`--no-extensions`. In a child process, `PI_SUBAGENT_CHILD=1` makes the extension
register tracking only, with no tools or router, and acknowledge the contract
through Pi's extension event bus.

The old Pi spool and ephemeral feature become retired compatibility state. The
upgrade first reconciles every persisted Pi session, optionally imports usable
legacy lifecycle/lineage records from old spool files only when they name an
existing persisted session, records an explicit one-time gap for anything else,
then removes the spool writer/drain and owned directory. Existing database
`ephemeral` columns remain inert for schema compatibility, while production
writers, badges, and special query paths are removed.

Alternatives rejected:

- Returning to external live transcript inference: explicit custom entries avoid
  path/timing/model inference and preserve process/lineage truth.
- Keeping a failure-only spool: it duplicates persisted session evidence,
  creates an independent retention policy, and was the source of cross-process
  deletion and compatibility backlog.
- A separate extension sidecar journal: Pi already provides durable custom
  entries with session ownership and migration handling; another file adds no
  needed capability after `--no-session` is excluded.
- Supporting both exact-match and previous-protocol readers: the human selected
  exact pairs; mismatches fail typed and inert until corrected.
- Persisting heartbeats: durable files prove history, not current process
  liveness. Heartbeats remain live, bounded, and replaceable.
- Nested Sessions UI: descendants flatten into the existing rail by decision.

Constitution alignment:

- Principle 1: persisted Pi entries are authoritative; missing/unflushed files,
  malformed custom entries, and unresolved lineage remain explicit gaps.
- Principles 2 and 3: existing Rust/Tauri storage, source coordinator,
  `LiveTracker`, and Pi extension own the path; live handlers stay bounded and
  reconciliation runs off hot/UI threads.
- Principle 4: source replacement, migration, provider deployment, and legacy
  spool retirement are transactional and last-known-good preserving.
- Principle 5: compatibility, unknown-session, source-parse, reconciliation,
  reporter-loading, and transport failures are typed with contextual causes.
- Principles 6-8: the authorized owning-layer suites, full zero-warning gates,
  synchronized `lat.md`, and `lat check` are mandatory.
- Principle 9: existing Sessions and Integrations density, accessibility, and
  provider color remain unchanged except truthful fields/copy.
- Principle 10: handler, inventory, reconciliation, overlay, fleet, and recovery
  budgets are measured against explicit floors.
- Principle 11: custom entries and live pushes contain no message bodies and
  remain local; no off-device channel is added.
- Principle 12: exact-pair release, migration, external child contract, review,
  and validation remain gated through Beads.

## Affected Components

### Shared protocol and durable Pi entry contract

- `src-tauri/src/models.rs`
  - Add protocol-v2 raw/open envelope metadata, closed domain events, typed
    response codes, lifecycle disposition, persisted direct-lineage/agent
    fields, and per-reporter health types.
- `src-tauri/src/pi_tracking.rs` (new)
  - Own raw authentication-compatible decoding, exact-version checks, custom
    entry schema validation, lifecycle ordering/disposition, event receipts,
    reporter health aggregation, and table-driven tests.
- `src-tauri/pi-integration/quill.ts`
  - Export deterministic event builders; append `quill-tracking` entries before
    live delivery; maintain process instance/sequence; skip non-persistent
    sessions; remove spool/log paths; register tracking-only child behavior;
    canonicalize telemetry; implement exact-pair candidate registration.
- `src-tauri/pi-integration/quill.test.mjs`
  - Cover persistent/no-session boundaries, append-before-send, exact payload
    bytes, reload/resume occurrence identity, typed responses, broker election,
    child acknowledgement, telemetry mapping, and handler budgets.
- `src-tauri/pi-integration/package.json`,
  `src-tauri/pi-integration/README.md`,
  `.github/workflows/publish-pi-extension.yml`, and
  `scripts/pi-package.test.mjs`
  - Bump reporter/protocol contract, publish exact-pair compatibility, ship the
    generated fixture, and gate package release on the real Rust contract.

### Persisted Pi source parsing and reconciliation

- `src-tauri/src/pi_session.rs`
  - Parse versioned `quill-tracking` custom entries plus native Pi messages,
    model changes, tool calls/results, and usage from one stable session
    snapshot; preserve unknown custom entries without failure.
- `src-tauri/src/transcript_analytics.rs` and
  `src-tauri/src/transcript_identity.rs`
  - Extend the existing stable-read, fingerprint, source-generation,
    provider/root permit, retry/backoff, and last-known-good coordinator for Pi
    persisted sessions. Do not create a second reconciliation coordinator.
- `src-tauri/src/sessions.rs`
  - Restore Pi startup enumeration and changed-source admission, drive search
    indexing from the same stable snapshot, preserve provider-qualified path
    metadata, and keep notify as a fast path.
- `src-tauri/src/storage.rs`
  - Reserve the next available schema version; create a crash-safe SQLite
    pre-migration backup before migration begins; add source replacement methods;
    unify live/persisted event IDs; store lifecycle instance/direct lineage/
    agent role, bounded reporter health, source generation, and event receipts;
    remove active Pi dependence on failed-request replay and ephemeral writers.
- `src-tauri/src/transcript_watcher.rs` and `src-tauri/src/lib.rs`
  - Register the Pi root for bounded changed-source admission and periodic
    inventory; reuse/manage the shared source coordinator and keep work off
    setup/UI threads.

### Live state, Sessions projection, and health

- `src-tauri/src/server.rs`
  - Replace typed extractor-first `/api/v1/pi/track` with protocol-v2 raw
    handling; remove Pi spool drain/offsets; return lifecycle disposition and
    `unknown_session`; route committed applied mutations into `LiveTracker`;
    reuse persisted-source notify/reconciliation.
- `src-tauri/src/live_tracker.rs`
  - Rehydrate recovering Pi sessions, apply process-instance ordering, resolve
    arbitrary-depth direct lineage, preserve unresolved live rows, flatten
    descendants, aggregate the approved metrics, carry agent labels, and
    backfill visible page limits.
- `src-tauri/src/integrations/pi.rs`
  - Deploy/verify exact reporter bytes and protocol version, retire owned spool
    artifacts safely, preserve pre-upgrade DB backup evidence, remove ephemeral
    consent/copy, and verify child-contract capability.
- `src-tauri/src/integrations/manager.rs` and
  `src-tauri/src/integrations/types.rs`
  - Replace global last-writer health with per-reporter/process dimensions and a
    worst-state aggregate; enforce disable for every install channel without
    mutating user-owned packages.
- `src/types.ts`, `src/utils/format.ts`,
  `src/components/widget/views/UsageView.tsx`, and
  `src/mocks/ipcFixtures.ts`
  - Carry bounded agent role/unresolved-live state, preserve the approved metric
    split, remove the EPHEMERAL badge/path, and keep existing layout and
    accessibility grammar.
- `src/components/settings/IntegrationsTab.tsx`
  - Render exact-version mismatch, missing child reporter, recovering source,
    reconciliation gap, affected reporter count, remediation, and verified
    recovery without a new inspector/dashboard.

### Child reporter integration and release evidence

- `scripts/pi-subagents-tracking-contract.test.mjs` (new)
  - Launch real direct/nested children with ambient extensions enabled and
    disabled, verify explicit injection/acknowledgement, parent/root IDs, role,
    process instance, exact reporter pair, and tracking-only tool surface.
- External `pi-subagents` release dependency
  - Consume the parent reporter descriptor and inject the active Quill extension
    path as a child runtime extension. Quill completion requires a compatible
    released version; arbitrary launchers remain out of scope.
- `scripts/dev-runtime-isolation.mjs`
  - Add opted-in project/development reporter scenarios and prove they cannot
    overwrite or report to production without explicit selection.
- `lat.md/data-flow.md`, `lat.md/infrastructure.md`, `lat.md/backend.md`,
  `lat.md/features.md`, `lat.md/frontend.md`, and the existing `lat.md/pi-*.md`
  test-spec files
  - Supersede push-only/spool/ephemeral claims with persisted-session source
    truth, exact-pair compatibility, reconciliation, live acceleration, and the
    clarified agent projection.
- `release_notes.md` and `README.md`
  - Document exact pairing, required Pi reload, no `--no-session` coverage,
    migration/rollback, old spool retirement, and recovery behavior.

## Data Model

Use one forward-only schema migration. Keep existing additive `ephemeral`
columns for downgrade/schema stability but stop writing or rendering them.

### `pi_session_lifecycle`

One current durable lifecycle state per canonical
`(provider, normalized_hostname, session_id)` plus retained occurrence identity:

- `provider`, `normalized_hostname`, `session_id`, `source_key`
- `origin_at_ms`
- `process_instance_id`
- `current_sequence`
- `current_occurrence_id`
- `occurred_at_ms`
- `lifecycle_state`: `open | closed | recovering`
- `direct_parent_session_id`, nullable
- `visible_root_session_id`, nullable
- `lineage_state`: `root | linked | agent | unresolved`
- `lineage_reason`, nullable
- `agent_role`, nullable and bounded
- `reporter_protocol`, `reporter_version`
- `updated_at`, `closed_at_ms`, nullable

An accepted start with a new process instance supersedes the previous instance.
Within one instance, higher sequence wins, equal sequence is duplicate, and
lower sequence is stale. Activity/end from a superseded instance is stale.
Only explicit start/reannounce changes closed/recovering state to open.

### `pi_event_receipts`

Dedupe and live/reconciliation overlap by:

- `(provider, normalized_hostname, session_id, event_uuid)` primary identity
- `source_key`, `entry_id`, `process_instance_id`, `sequence`
- `event_kind`, `occurred_at_ms`, `accepted_at_ms`

The custom entry's event UUID is the same live HTTP UUID. Reconciliation can
replace source-owned rows while retaining receipt identity, so accepted live
usage/runtime cannot double-count and a stable source can correct it.

### `pi_reporter_health`

Bounded per-reporter/process dimensions:

- normalized host, process instance, install channel, reporter/protocol/build
  version; primary key `(normalized_hostname, process_instance_id,
  install_channel)`
- last handshake, last known-session acceptance, last heartbeat
- connection, compatibility, lifecycle, child-ack, source-reconciliation, and
  transport states
- latest typed code, affected session count, recovered/resolved timestamps

The integration summary returns the worst active state, affected count, latest
reason, and last verified recovery. Same-reporter success clears transient
transport state; source errors clear only when that source reconciles. A row
without heartbeat expires from active health after 15 minutes; terminal or
recovered rows are retained for 24 hours, then summarized and deleted. Cap
active reporter rows at 4,096 and terminal rows at 4,096 per host; saturation
records one typed aggregate overflow instead of evicting active failures.

### Persisted custom entry schema

```json
{
  "type": "custom",
  "customType": "quill-tracking",
  "data": {
    "schema": 2,
    "event_uuid": "...",
    "event": "session_start | session_end | lineage",
    "session_id": "...",
    "process_instance_id": "...",
    "sequence": 1,
    "origin_at": "RFC3339",
    "occurred_at": "RFC3339",
    "reason": "startup | reload | new | resume | fork | quit",
    "delivery_source": "live | reconciliation",
    "lineage": {
      "kind": "root | linked | agent | unresolved",
      "parent_session_id": "...",
      "reason": "..."
    },
    "agent_role": "reviewer",
    "reporter": { "protocol": 2, "version": "..." }
  }
}
```

Start reasons are `startup | reload | new | resume | fork`; end reasons are
`quit | reload | new | resume | fork`. Replacement is represented by
`previous_session_id`, revival is a new process instance using Pi's emitted
start reason, and `live | reconciliation` describes delivery rather than a
lifecycle reason. Optional fields are omitted, not null. Unknown `customType`
values and future tracking schemas remain preserved in the Pi file and produce
a typed exact-pair reconciliation error rather than file mutation or deletion.
Agent roles are
trimmed, control-free, bounded, launcher-declared values; task/prompt-derived
freeform labels are rejected.

### Source ownership

One `pi:session:<normalized-host>:<session-id>` source owns persisted and live
Pi evidence; provider `pi` remains explicit in every table and wire key. Native
Pi entry IDs and custom tracking event UUIDs form stable action/runtime/usage
keys. Reconciliation replaces the source snapshot atomically; live pushes may
append same-key in-flight evidence between snapshots. The rollup write permit,
retention watermark, and session-source open-tail rules remain shared with the
existing Pi live source.

Legacy protocol-1 spool files are not an evidence source. After the required Pi
reload stops legacy writers and persisted-session reconciliation succeeds, the
upgrade records one typed retirement gap/count, removes only dead or
rename-claimed owned spool artifacts, and marks the spool retired. It imports no
lifecycle, lineage, usage, or runtime records from that second source. No new
spool or quarantine table is created.

## API / Interface Changes

### `POST /api/v1/pi/track` protocol v2

Handler receives bounded raw bytes, checks bearer auth before compatibility
detail, parses open metadata, enforces exact protocol/reporter pair, validates
the full envelope, commits it transactionally, then applies committed live
mutations.

Typed responses:

- `202`: `{status:"accepted", quill_build:"...", protocol:2,
  reporter_version:"...", capability_digest:"...",
  outcomes:[applied|duplicate|stale]}`
- `400`: malformed JSON, invalid fields, impossible lifecycle/lineage
- `401`: generic unauthorized, no version detail
- `409`: `unknown_session` or `reannounce_required`
- `426`: exact protocol/reporter/Quill mismatch with required versions
- `429`: rate limited with retry guidance
- `503`: quiesced/transiently unavailable

No authenticated permanent response creates a failed-request spool. The
extension logs one bounded typed error and retries only timeout/429/503; 401
reloads config once; 409 appends/reannounces lifecycle then retries once; 426
stays inert until reload with an exact pair.

### Persisted-source reconciliation

- Startup, provider re-enable, 120-second backstop, and watched changes inventory
  the configured Pi session root.
- `/api/v1/sessions/notify` remains a changed-source fast path and accepts the
  exact persisted-source identity; it is no longer the only discovery path.
- Pi `/sessions/messages` and model/usage live pushes remain optional live hints
  using source-stable native/custom IDs. Reconciliation is authoritative and
  corrects missing or partial live hints.
- A stable parse replaces search documents and source-owned analytics/lifecycle
  together where their existing transactions permit; independent index failure
  cannot delete committed analytics, and analytics failure retains last-good.

### Extension and child contract

- Root persistent sessions register full tools/routing plus tracking.
- `--no-session` registers tools/routing as configured but no tracking handlers
  or health subject.
- `PI_SUBAGENT_CHILD=1` with an injected exact reporter registers tracking only.
- Parent reporter descriptor includes extension path, protocol/version,
  capability digest, root session id, and opt-in trust source.
- Child acknowledgement returns reporter/capability digest, direct parent, root,
  process instance, and bounded role.
- Unsupported launcher/contract produces no invented lineage and remains outside
  the supported guarantee.

### Sessions and Integrations IPC

Additive fields expose:

- direct parent and visible root
- live unresolved reason
- bounded agent role
- recovering/open state
- per-reporter health summary, affected count, typed reason, exact required
  version, and last verified recovery

Remove active EPHEMERAL presentation and Pi-specific no-file tracking behavior.
No nested UI, agent explorer, or new health dashboard is added.

## Testing Strategy

Authorized tests pin invariants at their owning layer and link one-to-one with
updated `lat.md` specs.

- **Cross-wire fixture:** TypeScript builders generate exact protocol-v2 JSONL
  for every lifecycle reason, lineage kind, optional field, invalid value,
  response class, and exact-version/build mismatch. Pure P0 fixture tests pin
  bytes and decoder acceptance; after route/migration work, Rust feeds those
  same bytes through the authenticated real router, transaction, and response.
  Negative cases cover unauthenticated malformed JSON, malformed UTF-8,
  oversized bodies, excess event count, unknown old/new generations, and prove
  unauthenticated callers receive no compatibility detail. Successful `202`
  asserts Quill build, protocol, reporter version, capability digest, and event
  outcomes.
- **Pi extension:** persistent session appends before live send; buffered
  pre-assistant entry later flushes; `--no-session` writes/sends nothing;
  reload/resume/process replacement sequences stay distinct; 409 reannounce is
  bounded; 426 is inert; same IDs cross custom entry/live push; no spool/log
  directory is created; canonical telemetry and handler timing pass. Root
  persistent and `--no-session` modes retain the exact eight `quill_` tools and
  context-router cases, while injected children expose neither. Tracking
  payload tests reject prompt, message, tool-output, or non-loopback fields.
- **Parser/reconciliation:** v2/v3 Pi tree files, custom entry schema, unknown
  custom types, malformed tracking entry, partial trailing line, rewrite/drift,
  unchanged fingerprint, startup inventory, notify fast path, downtime-complete
  session, last-good preservation, removal/prune, and exact source replacement.
- **Lifecycle migration:** fresh and upgraded databases, reserved next schema
  version, SQLite backup created and verified before migration, equal/lower/
  higher sequences, superseded processes, start/end/reannounce, stale replay,
  live/reconciliation overlap, transaction rollback, event receipt dedupe,
  disable-during-reconciliation, and old spool retirement without importing it.
  Fault cases cover WAL/backup failure, disk full, source deletion, and restart
  at each cutover checkpoint.
- **Live graph:** missing parent live fallback, completion removal, late attach,
  direct and depth-64 descendants, cycles, cross-host conflict, depth overflow,
  root ranking, page backfill, stable ordering, agent roles, and the clarified
  metric split.
- **Health:** concurrent healthy/incompatible reporters, exact version detail,
  known-session versus unknown-session acceptance, child acknowledgement,
  recovering/reconciled source, same-reporter error clearing, worst-state
  aggregation, disable/re-enable, and no subject for `--no-session`.
- **Reporter broker/deployment:** managed/npm/project candidates, opted-in dev,
  legacy pre-broker reload requirement, valid foreign overwrite,
  newer-reporter/older-server and older-reporter/newer-server rejection,
  mid-process server drift, feature flags, user-owned files, transactional
  repair/rollback, package tag, npm provenance, and matching Pi/Node support.
- **Real `pi-subagents`:** direct, parallel, async, nested/fanout, resume, and
  ambient-disabled launches inject/acknowledge tracking-only reporter and expose
  no `quill_` tools/router in children.
- **Fleet qualification:** 64 concurrent persisted child launches; 1,000
  events/minute for ten minutes; zero lost lifecycle starts after
  reconciliation; sync handler maximum `<=10 ms`; 1,500 ms local timeout;
  at least 4x server headroom; Sessions read maximum `<=300 ms`; recovery within
  31.5 seconds; no more than 10% p95 regression for event-loop delay, source
  reconciliation, overlay, or RSS. Record session-file growth, changed-source
  backlog depth/age, retry counts by typed cause, database/WAL growth, and
  reconciliation throughput against the pre-change baseline.
- **UI/accessibility:** unresolved rows live-only, completion removal, late
  attach, flat root rail, role text, recovering/mismatch health copy, affected
  counts, exact required version, long names, keyboard/focus, compact density,
  and EPHEMERAL absence in isolated fixtures without touching the live window.
- **Removal audit:** no Pi spool writer/drain/offset/cap code, no active
  ephemeral writer/query/UI branch, no push-only discovery assumption, no
  duplicate telemetry mapping, and no stale Feature 027 `lat.md` contract.

Full gates: focused Node/Rust/UI suites, real Pi 0.84 minimum and current
supported runs, real `pi-subagents` contract, `cargo fmt --check`, Clippy with
warnings denied, Rust checks/tests, npm test/lint/build/knip, package tarball,
`git diff --check`, `lat check`, and exact-pair release dry run.

## Risks

- **Pi custom entries change the session tree.** They advance the leaf even
  though they are absent from model context. Mitigation: persist only lifecycle
  and lineage occurrences, never heartbeat/activity noise; test branch/fork/
  compaction behavior and unknown custom-entry preservation.
- **Pre-assistant crash has no durable file.** Accepted by clarification: no
  alternate journal, spool, health row, or synthetic gap record. This is an
  intentional absence limited to work Pi itself never persisted, documented in
  support and release behavior.
- **Full Pi inventory may be expensive.** Mitigation: fingerprints,
  source-generation short circuit, stable bounded reads, existing coordinator
  backoff, one-source-at-a-time memory, and measured inventory/reconciliation
  budgets.
- **Tree rewrites can race reconciliation.** Mitigation: stable snapshot checks,
  content fingerprint, generation ordering, and last-known-good retention.
- **Live and persisted writers can conflict.** Mitigation: identical stable IDs,
  one session-owned source, event receipts, rollup write permit, and atomic
  replacement tests.
- **Exact version pairing increases operational friction.** Mitigation: server
  and managed extension ship atomically, npm publication follows desktop,
  integration health names exact required versions, and release notes require
  Pi reload.
- **Legacy reporters cannot join the broker.** Mitigation: explicit reload or
  removal, server-side duplicate tolerance during upgrade, and no claim that
  hot replacement can unregister old handlers.
- **`pi-subagents` is an external release dependency.** Mitigation: freeze a
  small versioned contract, publish Quill's reporter descriptor first, gate
  Quill completion on a compatible release, and keep arbitrary launchers out of
  scope.
- **Forward-only migration constrains rollback.** Mitigation: pre-upgrade DB
  backup evidence, exact release sequencing, additive/inert old columns, and
  forward-fix preference.
- **Historical Pi ephemeral and push-only rows remain.** Mitigation: stop new
  Pi production, keep schema readable, remove only Pi no-file presentation and
  writers, preserve non-Pi semantics, avoid reattribution, and disclose that
  pre-feature evidence remains under its prior semantics.
- **Sensitive local metadata persists in Pi files.** Mitigation: bounded
  launcher roles only, no prompt/task names, 0600 Pi session permissions,
  payload-free logs, and search/support-export exclusion.

## Sequencing

Every item below materializes as one child task under the new epic **Harden Pi
agent tracking**. Priorities P0-P2 are within the pipeline's allowed P0-P3
range. Each task updates its named `lat.md` sections and runs `lat check`; final
qualification audits rather than becoming the first documentation owner.
Every description/acceptance must repeat the exclusion: do not stage, rewrite,
or absorb work owned by active task `quill-9swu`.

### Capture the Pi tracking baseline — P0

Read-only first task. Add one reproducible harness before behavior changes and
append baseline evidence to this spec.

Files: `scripts/pi-agent-tracking-baseline.mjs`, `package.json`,
`specs/028-pi-agent-tracking-hardening.md`.

Acceptance:

- Records current extension handler/event-loop latency, real/synthetic fleet
  request rate, RSS, Pi session-file growth, changed-source inventory cost,
  reconciliation backlog, Sessions read/overlay latency, database/WAL growth,
  and current failures under 64 children.
- Reports sample count, environment/profile, median, nearest-rank p95, maximum,
  and exact command; stores no prompts, content, session IDs, paths, or host.
- Produces pass/fail baselines for later `<=10%` comparison and changes no
  runtime source or live Quill window.

Focused command: `node scripts/pi-agent-tracking-baseline.mjs`.

Blocks every behavior-changing task.

### Freeze protocol-v2 and persisted-entry contracts — P0

Depends on baseline. Foundational shared primitive. Owns open-envelope and
closed-domain types, exact Quill/reporter/build constants, successful handshake
metadata, lifecycle/lineage state split, canonical
`(provider, normalized_host, session_id)` identity, custom-entry schema, typed
responses, deterministic TypeScript fixture generation, and pure Rust decoder
acceptance. Older/newer protocols are rejection fixtures only.

Files: `src-tauri/src/models.rs`, `src-tauri/src/pi_tracking.rs` (new),
`src-tauri/pi-integration/quill.ts`,
`src-tauri/pi-integration/quill.test.mjs`,
`src-tauri/pi-integration/fixtures/protocol-v2.jsonl` (new),
`lat.md/pi-extension-tests.md`, `lat.md/pi-live-session-tests.md`.

Acceptance:

- Fixture bytes cover every start/end reason, delivery source, lineage state,
  optional field, invalid field, exact mismatch, and typed outcome.
- Pure Rust decoder accepts only the exact generation; fixture regeneration is
  deterministic and CI-visible.
- Custom entries contain no prompt/message/tool output and successful handshake
  returns Quill build, protocol, reporter version, and capability digest.

Focused commands: `node --test src-tauri/pi-integration/quill.test.mjs` and
`cargo test --manifest-path src-tauri/Cargo.toml pi_tracking::tests`.

Blocks all remaining implementation tasks.

### Persist tracking evidence in Pi sessions — P1

Depends on contract. Refactor the extension to append lifecycle/lineage custom
entries before live push, derive stable native IDs, intentionally omit tracking
for `--no-session`, maintain process instance/sequence across reload, implement
409/426 behavior, expose broker/child descriptors, and stop creating the
failed-request spool. Preserve the exact eight root `quill_` tools and router;
injected children register tracking only.

Files: `src-tauri/pi-integration/quill.ts`,
`src-tauri/pi-integration/quill.test.mjs`,
`src-tauri/pi-integration/package.json`,
`src-tauri/pi-integration/README.md`,
`lat.md/pi-extension-tests.md`, `lat.md/infrastructure.md`.

Acceptance:

- Persistent custom entry is appended before same-ID live send and flushes with
  Pi's first assistant entry; missing/nonpersistent session writes and sends
  nothing and creates no health subject.
- Timeout/429/503 retry, 401 reload-once, 409 reannounce-once, and 426 inert
  behavior are bounded and tested; no Pi spool/log directory is created.
- Root persistent and no-session modes retain exactly eight tools/router cases;
  child mode exposes none; all tracking URLs are loopback and payload-free.

Focused command: `node --test src-tauri/pi-integration/quill.test.mjs`.

Blocks persisted-source reconciliation and broker deployment.

### Reconcile persisted Pi sources and migrate ownership — P1

Depends on extension persistence. Extend the existing transcript source
coordinator; do not create a second coordinator. Before the next schema
migration, create and verify a crash-safe SQLite backup from schema 45, then
migrate canonical identity, lifecycle/lineage split, receipts, bounded health,
and source ownership. Parse custom/native entries, restore Pi startup inventory,
replace lifecycle/runtime/usage/tool/skill/search evidence atomically, and
rehydrate durable open rows as recovering.

Files: `src-tauri/src/pi_session.rs`, `src-tauri/src/sessions.rs`,
`src-tauri/src/transcript_analytics.rs`,
`src-tauri/src/transcript_identity.rs`, `src-tauri/src/storage.rs`,
`src-tauri/src/transcript_watcher.rs`, `src-tauri/src/lib.rs`,
`lat.md/data-flow.md`, `lat.md/backend.md`,
`lat.md/pi-session-parser-tests.md`, `lat.md/pi-notify-index-tests.md`,
`lat.md/pi-model-usage-tests.md`.

Acceptance:

- Reuses existing stable reads, fingerprints, source generations, permits,
  retry/backoff, and last-good retention; no duplicate coordinator exists.
- Backup completes before DDL, includes DB/WAL state, has a literal restore and
  verification command, and interruption at each checkpoint remains resumable.
- Startup/notify/periodic reconciliation recovers a downtime-complete persisted
  session, rejects drift without replacing last-good rows, and uses canonical
  host-qualified source keys.
- Old spool records are never imported. Cleanup stays pending until exact
  reporter reload and successful source reconciliation.

Focused commands: targeted Pi parser/reconciliation/storage migration tests,
then `cargo test --manifest-path src-tauri/Cargo.toml pi_` and `lat check`.

Blocks transactional live projection and deployment cutover.

### Make lifecycle and nested-agent projection transactional — P1

Depends on reconciliation. Replace extractor-first live ingestion, feed exact
fixture bytes through the authenticated real router, use committed lifecycle
disposition for live mutation, implement heartbeat/unknown-session recovery,
resolve depth-bounded lineage, keep unresolved rows live-only, flatten
 descendants, apply the approved metric split, carry roles, preserve roots, and
backfill page limits. Enumerate unknown-session behavior for `/pi/track`, Pi
`/sessions/messages`, model/usage hints, and notify.

Files: `src-tauri/src/server.rs`, `src-tauri/src/pi_tracking.rs`,
`src-tauri/src/live_tracker.rs`, `src-tauri/src/storage.rs`,
`src-tauri/src/models.rs`, `src/types.ts`, `src/utils/format.ts`,
`src/components/widget/views/UsageView.tsx`, `src/mocks/ipcFixtures.ts`,
`lat.md/features.md`, `lat.md/frontend.md`,
`lat.md/pi-live-session-tests.md`, `lat.md/pi-lineage-ui-tests.md`.

Acceptance:

- Real router returns typed 400/401/409/426/429/503 and successful handshake
  metadata; unknown or unauthenticated shapes never become bare 422.
- Durable `applied|duplicate|stale|unknown_session` result and live mutation are
  one committed decision; stale reconciliation cannot reopen a newer process.
- Missing parent, late attachment, completion removal, depth 64, cycles,
  cross-host conflict, ranking, page backfill, role, and metric-split tests pass.
- The inherently unobservable pre-flush/no-file case produces intentional
  absence, while any live-observed missing source becomes a typed
  `source_not_persisted` diagnostic cleared on flush/reconcile.

Focused commands: real router/lifecycle/live-tracker tests and focused frontend
formatter/row tests, then `lat check`.

Blocks broker, child integration, and telemetry/health.

### Ship exact-pair reporter broker and retire legacy deployment — P1

Depends on extension persistence, reconciliation, and transactional lifecycle.
Implement candidate broker/election, exact managed/npm/project/development
policy, disable behavior, valid foreign overwrite handling, legacy reporter
reload detection, package versioning, and release ordering. Own the cutover:
restore extension bytes on failure; require Pi reload; stop legacy writer/drain;
remove only dead or rename-claimed spool artifacts after reconciliation; record
one retirement gap/count; never import spool evidence.

Files: `src-tauri/pi-integration/quill.ts`,
`src-tauri/src/server.rs`, `src-tauri/src/integrations/pi.rs`,
`src-tauri/src/integrations/manager.rs`,
`src-tauri/src/integrations/types.rs`,
`src-tauri/pi-integration/package.json`,
`.github/workflows/publish-pi-extension.yml`,
`scripts/pi-package.test.mjs`, `scripts/dev-runtime-isolation.mjs`,
`lat.md/infrastructure.md`, `lat.md/pi-lifecycle-tests.md`,
`lat.md/pi-package-tests.md`, `lat.md/pi-spool-tests.md`.

Acceptance:

- Exact mismatch in both directions, mid-process drift, managed/npm/project
  coexistence, opted-in dev, legacy pre-broker, disable/re-enable, user-owned
  files, transactional repair, DB backup restore, extension-byte restore, and
  required Pi reload pass.
- Old drain/writer cannot race cutover; only owned dead/claimed artifacts are
  removed after persisted reconciliation; rollback reopens the backup and
  verifies exact old reporter/server behavior.
- Desktop/managed reporter ships first; matching npm publication is gated on
  available desktop build and dry-run provenance checks.

Focused commands: Pi lifecycle/package/dev-isolation suites, package dry run,
rollback fixture, and `lat check`.

Blocks external child integration and telemetry/health.

### Pin the tracking-only pi-subagents release contract — P1

Depends on broker. Publish the reporter descriptor/ack contract, add real
direct/nested/ambient-disabled fixtures, and obtain a compatible external
`pi-subagents` release that injects the active extension path. Record exact npm
package version, registry tarball integrity/SHA-256, upstream repository commit,
release URL, minimum Pi version, acquisition command, and contract capability
digest in the test fixture and spec. No unpinned local package satisfies this
bead.

Files: `src-tauri/pi-integration/quill.ts`,
`scripts/pi-subagents-tracking-contract.test.mjs` (new),
`src-tauri/pi-integration/README.md`,
`src-tauri/pi-integration/package.json`,
`lat.md/pi-extension-tests.md`, `lat.md/pi-live-session-tests.md`,
`specs/028-pi-agent-tracking-hardening.md`.

Acceptance:

- Pinned released artifact passes direct, parallel, async, nested/fanout,
  resume, and ambient-disabled launch tests with tracking-only acknowledgement,
  exact versions, parent/root IDs, role, process instance, and no child tools/
  router.
- Capability ceiling semantics remain intact and arbitrary launchers remain
  explicitly unsupported.

Focused command: `node --test scripts/pi-subagents-tracking-contract.test.mjs`
against the pinned artifact.

Blocks telemetry/health and final qualification.

### Canonicalize telemetry and bound reporter health — P2

Depends on lifecycle, broker, and pinned child acknowledgement. Remove duplicate
or undefined hook/runtime evidence, check non-2xx responses, store bounded
per-reporter health, render worst-state/affected-count/remediation/recovery, and
remove only Pi no-file/EPHEMERAL production presentation while preserving
historical and non-Pi semantics.

Files: `src-tauri/pi-integration/quill.ts`,
`src-tauri/src/pi_tracking.rs`, `src-tauri/src/storage.rs`,
`src-tauri/src/integrations/manager.rs`,
`src-tauri/src/integrations/types.rs`,
`src/components/settings/IntegrationsTab.tsx`, `src/types.ts`,
`src/components/widget/views/UsageView.tsx`, `src/utils/format.ts`,
`src/mocks/ipcFixtures.ts`, `lat.md/data-flow.md`,
`lat.md/pi-integrations-ui-tests.md`, `lat.md/pi-extension-tests.md`.

Acceptance:

- One canonical Pre/Post pair, no undefined turn hook, correct Stop/text
  semantics, and typed non-2xx health pass.
- Health key/expiry/caps are enforced: 15-minute active expiry, 24-hour
  terminal retention, 4,096 active and terminal rows per host, typed saturation,
  same-subject recovery clearing, and worst-state aggregate.
- Exact mismatch, unknown session, missing child ack, recovering source,
  reconciliation failure/recovery, and no-session absence render accessible
  existing-density copy without a new dashboard.

Focused commands: extension telemetry, health storage/manager, and isolated UI
suites, then `lat check`.

Blocks final qualification.

### Qualify the complete exact-pair release and synchronize docs — P2

Depends on every prior task. Evidence-only gate: run real cross-wire, migration,
persisted reconciliation, directional deployment-skew, mid-process drift,
rollback, 64-child fleet, performance, UI/accessibility, removal, package, and
full quality suites. Compare against the baseline and append final evidence to
this spec. Rewrite remaining superseded Feature 027 claims, release notes, and
README. If a gate exposes a defect, create a focused child bead with file
ownership and dependency back to this task; do not patch implementation files
inside qualification.

Files: `specs/028-pi-agent-tracking-hardening.md`, `release_notes.md`,
`README.md`, remaining named `lat.md/` Pi sections/test specs, and qualification
scripts/fixtures only. Do not touch implementation files or files/changes owned
by `quill-9swu`.

Acceptance:

- Every user story, clarification, constitution principle, exact-pair release,
  rollback, removal, and external receipt has direct evidence.
- Fleet report includes all required metrics and each p95 regression is <=10%;
  hard floors and 31.5-second recovery pass with zero lost reconciled starts.
- Repo-wide removal audit finds no spool writer/drain/offset/cap, active Pi
  ephemeral branch, push-only discovery assumption, duplicate telemetry, or
  stale contradictory `lat.md` claim.
- Full zero-warning gates, `git diff --check`, package tarball/provenance dry run,
  exact-pair release dry run, and `lat check` pass.

Focused commands: the full Testing Strategy command set plus
`node scripts/pi-agent-tracking-baseline.mjs --compare`.

Dependency graph:

- Baseline is the sole root.
- Contract follows baseline.
- Extension persistence follows contract.
- Reconciliation follows extension persistence.
- Transactional lifecycle follows reconciliation.
- Broker/cutover follows extension persistence, reconciliation, and lifecycle.
- Pinned child contract follows broker.
- Telemetry/health follows lifecycle, broker, and child acknowledgement.
- Final qualification follows all prior work.

Serialization is intentional: `quill.ts`, `storage.rs`, shared protocol types,
and `lat.md` leaves are repeatedly owned. No two unordered tasks modify the same
new primitive or file.

## Backlog Refinement

None. No open P4 source exists in the hierarchy/provenance scope. Completed
`quill-bjjo`, Features 021/022, and Feature 027 remain closed historical source
context. Materialization creates the new epic and one dependency-wired task for
each Sequencing item at P0-P2, all within the allowed P0-P3 range, without
reopening or superseding completed work.

## Alignment fixes applied

- Reconciled exact-pair compatibility throughout: one accepted protocol/build/
  reporter generation, explicit old/new rejection fixtures, exact handshake
  metadata, and no previous-generation reader.
- Split lifecycle state from lineage resolution; froze start/end reason mapping,
  delivery source, process-instance sequence ordering, and canonical
  provider/host/session identity across tables, receipts, source keys, live
  state, notify, and pagination.
- Removed legacy spool import entirely. Persisted Pi sessions are the sole
  source; cutover records a retirement gap/count and removes only dead/claimed
  owned artifacts after exact reporter reload and successful reconciliation.
- Reused the existing transcript stable-read/source coordinator instead of
  creating a second `pi_reconcile` generation/permit system.
- Added a P0 pre-change baseline harness and made every later performance gate a
  reproducible comparison with explicit required fleet metrics.
- Added crash-safe pre-migration database backup before DDL, resumable cutover
  ordering, literal restore evidence, and single ownership for writer/drain
  retirement.
- Converted broad workstreams into materializable P0-P2 epic-child tasks with
  exact files, acceptance assertions, focused commands, evidence, and dependency
  edges; all priorities remain inside the allowed P0-P3 range.
- Serialized all repeated `quill.ts`, `storage.rs`, protocol, child-ack, health,
  and `lat.md` ownership; child integration now blocks telemetry/health.
- Added a pinned external `pi-subagents` release receipt with exact package,
  tarball integrity, commit, capability digest, and deterministic acquisition.
- Bounded reporter-health identity, expiry, row caps, saturation behavior, and
  summary cleanup.
- Defined pre-assistant/no-file behavior as intentional absence rather than an
  impossible durable gap record.
- Added direct privacy, loopback, exact eight-tool/router, directional skew,
  mid-process drift, backup/WAL, disk-full, fleet growth/backlog/retry, and
  Pi-only ephemeral-removal assertions.
- Made final qualification evidence-only; discovered defects create focused
  blocking beads rather than unowned implementation edits.
- Repeated the `quill-9swu` staged-work exclusion in materialization and final
  diff ownership.
