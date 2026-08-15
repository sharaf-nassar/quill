# pi-tracking-extension

## Problem Statement

Quill's Pi integration (spec 026) split responsibilities: a managed extension
(`quill.ts`) provides context tools and observe-only hook pings, while ALL
session, agent, token, and lineage tracking comes from scraping Pi's session
JSONL transcripts (`pi_session.rs`, transcript watcher, live-tracker Pi fold,
`model_usage.rs` Pi adapter). That scraping path is structurally flaky for Pi
specifically, because Pi transcripts are rewritten trees, not appended logs:

- Pi is the only provider needing an `(mtime_ns, len)` fingerprint, a
  disabled `length == offset` fast path, and discard-and-rebuild full refolds
  (`live_tracker.rs:216`, `:292`, `:349`). A rewrite preserving mtime
  granularity and length is invisible.
- Session identity can change under a stable path (`replaced_session`,
  `live_tracker.rs:278`) — a case no other provider can hit.
- Lineage is a filesystem-path chain re-resolved by reading a header per hop
  on every fold (`pi_session.rs:204-238`); any broken hop silently degrades
  to "no lineage", indistinguishable from genuinely unlinked.
- Transcript files appear late (no file at session start) and ephemeral
  sessions never produce a file — permanently invisible today.
- The session root can move after install, requiring a 120 s re-resolve/
  re-watch retry loop (`transcript_watcher.rs:17`) and a placeholder
  fallback root (`sessions.rs:383`).
- Session lookup by id is an O(n) header scan over every transcript
  (`sessions.rs:4753`) because Pi filenames carry no identity.
- A Pi-only install is silently dead: `pi::install` never writes
  `~/.config/quill/config.json`, which `quill.ts` requires; the extension
  self-disables and registers zero tools and zero telemetry while `verify()`
  reports the install healthy.
- The extension swallows every failure in bare `catch {}` blocks; Pi API
  drift degrades to zero telemetry with no error surfaced anywhere.

Pi's in-process extension API can carry all of this properly, push-based:
`session_start`/`session_shutdown` (with new/resume/fork reasons and
`previousSessionFile`/`targetSessionFile`), `agent_start`/`agent_settled`,
`turn_start`/`turn_end`, `message_end` (full per-message `usage` including
cost, `provider`, `model`), `tool_execution_start/end`, `model_select`,
`session_compact`/`session_tree`, and `ctx.sessionManager` identity (id,
file, cwd, `parentSession`). Replace the scraping architecture with one
production-quality Quill Pi extension that reports all session and agent
tracking Quill requires, and completely remove the transcript-scraping
approach. The extension is intended for public release (npm pi package), so
production quality — health visibility, versioned compatibility, typed
failure handling, real-Pi tests — is a requirement, not a nicety.

## Goals

- One production-grade Quill Pi extension is the sole source of Pi session
  and agent tracking: session lifecycle (start/end/resume/fork, liveness),
  agent-run and turn boundaries, tool-execution activity, model identity and
  switches, token usage — pushed to Quill's local HTTP API as events happen.
- Live tracking gains what scraping could not deliver: immediate session
  start (no waiting for a transcript file), live cumulative token counts for
  Pi (removing the rendered `—` gap), deterministic session end on
  `session_shutdown` (faster than file-quiescence idle timeout), and
  ephemeral-session visibility (events fire with no file).
- Session lineage comes from the extension, resolved in-process once at
  session start (from `getHeader().parentSession` + `session_start`
  fork/resume provenance) and reported as stable header ids — no per-fold
  filesystem chain walking in Quill.
- Token/model usage is event-pushed per assistant message with the usage
  Pi computes (input/output/cache read/cache write, and cost — Pi supplies
  cost natively, unlike the scraped path), deduplicated so replays and
  reconnects cannot double-count.
- The transcript-scraping tracking path is completely removed: the
  live-tracker Pi fold and its rewrite-fingerprint machinery, the watcher's
  Pi root + retry loop, the Pi model-usage transcript adapter, and the
  per-fold path-chain lineage resolution all go, along with their tests and
  lat.md specs (superseded by extension-tracking specs).
- Coverage gaps are closed by design, not accepted silently: the extension
  appends unsent events to per-session spool files (identifiers, usage
  numbers, and lifecycle metadata only — never message bodies) and Quill
  itself drains the spool directory on startup/connect, so sessions run
  while Quill is closed — including ephemeral ones — still reach Quill.
- Search indexing stays transcript-based through a deliberately kept
  narrow parser: the extension pushes `sessions/notify` (transcript path +
  identity) and the indexer reads the named file, replacing the watcher
  root, its retry loop, and the O(n) id scan.
- Pi joins runtime/turn analytics this release: extension turn, message,
  and tool events feed the session-events/response-times pipeline so Pi
  appears in the Runtime card and per-turn latency surfaces, ending the
  `UnsupportedProvider` exclusion.
- The extension source is production-ready for later public release:
  versioned event protocol, health/handshake visible in Quill's
  Integrations UI (runtime health, not just bytes-on-disk verification),
  typed and bounded failure handling instead of blanket `catch {}`, no
  blocking work on Pi's hot path, and a real-Pi load test. Packaging and
  npm publication are a follow-up spec; the managed file drop remains the
  only deployment vehicle in this release.
- The Pi-only-install dead-config bug is fixed: enabling Pi provisions the
  full config contract the extension needs (`config.json` with url, secret,
  hostname, context url), independent of Claude/Codex ever being enabled.
- Existing non-tracking extension duties (the eight `quill_*` context tools,
  context-preservation routing, routing telemetry) continue working
  unchanged through the rewrite.
- Install/uninstall/repair keep the existing transactional lifecycle
  (FileSnapshots transaction, mutation guard, stamp-gated repair, orphan
  sweep, AGENTS.md block), updated for the new extension payload.

## Non-Goals

- No change to Claude Code or Codex tracking architecture; their hook
  scripts and transcript watching stay as they are. Shared server-side
  ingestion may be extended (new endpoints/fields), not redesigned.
- No Pi participation in Limits, CPA quota, brevity, Memory Optimizer,
  learning, or restart orchestration — the existing exclusions stand.
- No support for Pi SDK/RPC embedding modes; the interactive CLI is the
  target (extension events fire in RPC mode too, but no RPC-specific work).
- No Windows support (consistent with the other CLI providers).
- No community-package dependencies (e.g. pi-subagents) — subagent-style
  tracking uses only what core Pi events and lineage prove.
- Already-indexed Pi history is not migrated or re-attributed; disable
  continues to keep indexed data. Usage analytics for sessions that ran
  before this upgrade remain whatever the old adapter ingested; any
  remainder is an explicit gap, never silently backfilled.
- No npm packaging, publication, gallery listing, or npm-coexistence
  policy in this release — that is a named follow-up spec. The extension
  source is written to be publishable (no separately maintained public
  fork later), but the managed drop is the only install path now.
- No cost display anywhere in this release: Pi cost is stored only;
  display is its own cross-provider feature.
- No generic multi-provider agent-events protocol; any new ingestion
  route is Pi-scoped and versioned.
- No linking of bash-spawned child pi processes via `PI_SESSION_ID` env
  inheritance; lineage-linked concurrent sessions remain the only
  agent-count proxy.
- Ephemeral sessions are never search-indexed (no transcript content
  exists to index).
- No Sessions/search UI redesign: the corpus source changes, the surface
  does not. No new analytics views beyond the Pi columns/rows existing
  runtime surfaces gain.
- No runtime-health UI for Claude/Codex hooks this release — the health
  surface is Pi-scoped.
- No modification of Pi behavior beyond the already-shipped
  context-preservation routing denials; tracking handlers stay observe-only.

## Backlog Inputs

None. No open P4 sources exist (`bd list --status=open` is empty; closure
computed over an empty set).

## Target Epic

No existing epic. This run creates the feature epic.

## User Stories

### 1. Extension-native live session tracking

As a Pi user, I want my live Pi sessions tracked from the extension's
lifecycle events, so that liveness is immediate, accurate, and immune to
transcript rewrites.

Acceptance criteria:
- `session_start` produces a live session row (provider `pi`, header id,
  cwd, hostname) before any transcript file exists; `session_shutdown` ends
  it deterministically; quiescence remains only as a fallback for crashed
  Pi processes that never sent shutdown.
- Session replacement flows (`/new`, `/resume`, `/fork`) map to the correct
  end-then-start row transitions using the event `reason` and
  `previousSessionFile`/`targetSessionFile` provenance; a resumed session
  reuses its stable header id.
- Model identity comes from `model_select` and per-message events, not from
  re-parsing transcripts; the live row shows current model/provider.
- Ephemeral sessions (no session file) appear as live sessions while
  running and persist as explicitly badged session rows driven by pushed
  lifecycle and usage events — no transcript anchor is ever invented, and
  they never enter the search index (constitution 1).
- The live tracker's Pi-specific fold, fingerprint, and replaced-session
  machinery is deleted; no Pi arm remains in the transcript-tail fold path.

### 2. Extension-native token and model usage

As a user, I want Pi token usage pushed per message from the extension, so
that analytics are complete, live, and correctly attributed without
transcript parsing.

Acceptance criteria:
- Each completed assistant message reports provider-qualified model and
  usage (input, output, cache read, cache write) exactly once; a documented
  dedupe key makes replay/retry idempotent end to end.
- Live cumulative session tokens render real numbers for Pi (the `—` gap
  and the `active-branch` token-scope special case are removed; pushed
  usage is spend-truth across branches by construction).
- Pi cost figures (computed by Pi per message) are ingested and stored;
  no UI displays cost in this release (clarified: store-only).
- The Pi transcript model-usage adapter and its active-branch marking are
  removed once parity is demonstrated on real sessions.

### 3. Session lineage and agent activity from events

As a user, I want fork/resume lineage and agent-run activity reported by
the extension, so that linked sessions and activity are proven by Pi itself
rather than reconstructed from file paths.

Acceptance criteria:
- At session start the extension resolves and reports the parent session's
  stable header id (from `parentSession`/`previousSessionFile`) at most
  once per session; Quill never walks path chains at fold time. Unresolvable
  parents report as explicitly unlinked.
- Live linked-session overlay and Sessions-view parent navigation behave
  as today (or better), driven by the pushed lineage.
- `agent_start`/`agent_settled` and `turn_start`/`turn_end` boundaries and
  `tool_execution_start/end` events feed the existing hooks/activity
  breakdown with provider `pi`, replacing the current partial `EVENT_MAP`
  (which loses agent boundaries and tool execution results today).

### 4. Production-quality extension engineering

As a Quill maintainer, I want the extension engineered for public release,
so that third-party Pi users can install it safely and Quill can trust its
health.

Acceptance criteria:
- A versioned event protocol (protocol version + extension version +
  minimum-Quill handshake) lets Quill detect and surface incompatibility
  as a typed integration status instead of silent self-disable.
- Failure handling is typed and bounded: expected failures (Quill closed,
  config missing, endpoint refused) degrade the specific capability with an
  internally visible reason; the extension never breaks Pi's load or blocks
  a turn (hard timeout, fire-and-forget or spooled sends, no unbounded
  buffering, no work in the extension factory beyond registration).
- Runtime health is observable: Quill's Integrations tab shows when the
  extension last reported / that it is alive, distinguishing
  "installed but never connected" from healthy (fixing the dead-install
  blind spot).
- The extension has its own test suite covering registration, every
  tracking handler, spool append/drain, protocol version mismatch, and a
  real-Pi load test pinned to the supported Pi version range.
- Enabling Pi provisions the full config contract the extension needs
  (`config.json` written through one shared writer used by all provider
  enables — fixing the Pi-only dead-install bug); repair heals drift, and
  the file persists on uninstall while any provider still needs it.

### 5. Pi runtime and turn analytics

As a user, I want Pi sessions counted in Quill's runtime and turn
analytics, so that active-LLM-time and per-turn latency views cover Pi
work like they cover Claude and Codex (clarified: 6B, in scope).

Acceptance criteria:
- Pushed Pi turn/message/tool events land as source-less (live)
  `session_events` and `response_times` rows through the existing
  `/sessions/messages` live-analytics path — no new runtime ingest
  endpoint and no transcript parsing. (The `UnsupportedProvider` arm is
  cosmetic; the real gate is the Claude+Codex-only reconciliation root
  list, which stays unchanged.)
- The Runtime card includes Pi in BOTH backfill states: the
  completed-rollup read path gains a source-less branch that folds live
  raw events over the same window and unions them into the rollup stats —
  without touching the audited rollup invariants (no relaxation of
  `source_key NOT NULL`, contiguous-rowid, or backfill guards). This also
  fixes the pre-existing invisibility of remote-session live rows after
  backfill completes.
- Turn latency rows exist for Pi: user side from `input`/`turn_start`,
  assistant identity from `turn_end`; `tool_execution_start/end` map to
  the tool-use/tool-result pair so tool waits use the long tool-wait
  window instead of truncating turns; the non-Codex 600 s response cap is
  extended for Pi (long Pi turns must still produce rows and count in
  `turn_count`).
- Event identity survives the extension's per-session teardown: the
  extension mints stable message/event ids (reload/new/resume/fork safe),
  ordinals are contiguous per message, and every event shares its
  message's exact timestamp, so insert-or-ignore replay from the spool is
  idempotent by construction.
- Gaps are explicit, never zeroed (constitution 1): Pi timelines carry no
  `asst_thinking` events (Pi exposes no thinking blocks) — declared, not
  silently absent; a turn with no `turn_end` (crash/quit mid-turn) is
  visibly open, not a phantom row; per-session runtime evidence
  (`active_runtime_secs`, agent runtime) either gains a live-source branch
  or renders explicitly unknown for Pi — decided at plan, never a false 0.
- Growth is bounded: retention never prunes source-less rows today, so the
  plan must either bound Pi's live-row volume with a measured estimate and
  explicit acceptance, or extend retention to live rows with a
  no-half-deleted-turn story.
- If pushed rows become rollup-visible, the persist path takes the rollup
  backfill write permit and honors the recognized-not-duplicated
  live-replacement invariant.

### 6. Clean removal of the scraping architecture

As a Quill maintainer, I want the superseded scraping path removed
completely, so that the codebase carries one Pi tracking architecture.

Acceptance criteria:
- Removed: live-tracker Pi fold + fingerprint + replaced-session eviction,
  watcher Pi root registration + 120 s retry + ambiguous-root Pi arm,
  Pi model-usage transcript adapter + active-branch turn marking,
  per-fold `resolve_pi_parent_session_id` path-chain resolution, and the
  O(n) `find_pi_session_path_in` scan — with whatever narrow parser subset
  the search-indexing decision (Open Question 1) still requires retained
  deliberately, not by inertia.
- Every removed behavior's lat.md spec section is superseded or rewritten;
  `lat check` passes; no orphaned `@lat:` refs remain.
- Integration state, stamps, and install/uninstall keep working across the
  upgrade: an existing Pi install upgrades in place via startup repair to
  the new extension payload without user action.
- A post-removal sweep proves no dead Pi-scraping code paths remain
  (no unreferenced Pi arms in watcher/live-tracker/model-usage).

## Constraints

- Pi extension API facts (verified against installed Pi 0.84.2, the latest
  release): extensions are in-process TypeScript loaded via jiti from
  `~/.pi/agent/extensions/` (global), `.pi/extensions/` (project,
  trust-gated), or settings/packages; default-export factory receiving
  `ExtensionAPI`; per-extension load-failure isolation; NO sandboxing —
  full user permissions; extension instances are torn down and re-created
  on `/reload` and on every session replacement (new/resume/fork), so no
  in-memory state survives across sessions; background resources must start
  in `session_start` and close idempotently in `session_shutdown`, never in
  the factory.
- Event surface available: `session_start` (reason:
  startup/reload/new/resume/fork, `previousSessionFile`),
  `session_shutdown` (reason: quit/reload/new/resume/fork,
  `targetSessionFile`), `session_before_switch/fork/compact`,
  `session_compact`, `session_tree`, `session_info_changed`,
  `before_agent_start`, `agent_start`, `agent_end`, `agent_settled`,
  `turn_start`/`turn_end` (turnIndex, message, toolResults),
  `message_start/update/end`, `tool_execution_start/update/end`,
  `tool_call` (blockable), `tool_result`, `model_select`,
  `thinking_level_select`, `input`, provider request/response hooks.
  `AssistantMessage.usage` carries input/output/cacheRead/cacheWrite/
  totalTokens plus per-field cost; assistant messages carry `provider`,
  `model`, `api`, `stopReason`, `responseId`.
- Session identity: `ctx.sessionManager.getSessionId()`, `getSessionFile()`
  (undefined = ephemeral), `getCwd()`, `getHeader()` →
  `{version?, id, timestamp, cwd, parentSession?}`; `parentSession` is a
  parent session file path. `pi.appendEntry()` persists custom entries to
  the session JSONL for extension state reconstruction on resume.
- Extensions may use `fetch()` and node builtins freely (no network
  restriction); all Quill I/O must be bounded/fire-and-forget — the
  extension runs on Pi's process and any blocking call stalls the user's
  agent (constitution 3). Existing local timeout convention: 1500 ms.
- Quill server posture: main router binds `0.0.0.0` deliberately; the
  context server is loopback-only on port 19877 gated by
  `context_http.enabled`. New tracking ingestion must follow the
  threat-model discipline (auth, validation, rate limits) of
  `POST /api/v1/hooks/observed`; existing endpoints available for reuse or
  extension: `hooks/observed`, `tokens`, `sessions/notify`,
  `sessions/messages`, `context-savings/events`.
- Current install machinery to preserve: FileSnapshots transactional
  deploy, integration mutation guard, stamp-gated startup repair,
  Quill-ownership markers (`quill-managed:pi`), orphan sweep, AGENTS.md
  managed block, min-Pi-version detection gate (currently 0.84.0; revisit
  against the events the new extension requires), demo-mode read-only.
- Feature gates today: `context_preservation`, `activity_tracking`,
  `context_telemetry` rendered into the extension via byte-exact
  placeholder substitution; the config-contract redesign (story 5) changes
  the delivery mechanism, and stamp semantics must follow.
- Pi is pre-1.0: extension API stability is not guaranteed. The versioned
  protocol/handshake (story 4) is the mitigation; supported-version policy
  must be explicit at detection and at runtime.
- Constitution: 1 (gaps explicit — ephemeral/offline coverage decisions
  must never invent data), 2 (extend existing seams — same provider enum,
  server, storage), 3 (no blocking on Pi's hot path; bounded background
  work), 4 (transactional install; spool durability), 5 (typed failure
  boundaries replacing `catch {}`), 6-8 (gates, authorized tests one-to-one
  with lat.md, lat sync), 10 (any hot-path budget claims measured), 11
  (extension transmits only to the local Quill API), 12 (gated delivery).
- Runtime-pipeline facts (verified for story 5): a push-based source-less
  lane already exists — `POST /api/v1/sessions/messages` →
  `store_live_session_analytics` writes live `session_events` and
  `response_times` with insert-or-ignore; live identity is
  `(provider, session_id, "{message_id}:{ordinal}")` /
  `(provider, session_id, chain_id, assistant_timestamp)`; the server
  enforces ordinal contiguity from 0 and exact message-timestamp equality
  per event; sidechain identity is validated all-or-nothing
  (`chain_id == agent_id`, `parent_chain_id == session_id`). Once runtime
  backfill completes, the Runtime card serves only source-owned rollups
  (joined to `transcript_analytics_sources`) — source-less rows become
  invisible, which is the central read-side change this feature makes.
  Live rows have no replacement/correction path and are exempt from
  retention; the `response_secs` cap is 600 s for non-Codex providers;
  five event kinds exist (`user_text`, `user_tool_result`, `asst_text`,
  `asst_thinking`, `asst_tool_use`) and only the
  tool-use→tool-result gap may exceed the 5-minute idle threshold.
- Prior art to follow: Codex `session-sync.cjs`/`report-tokens.cjs`/
  `hook-observe.cjs` push model; spec 026 and its lat.md test specs are the
  authoritative description of what exists.

## Open Questions

All ten questions below are resolved — product questions 1-6 by the human
at the clarify gate (see Clarifications), 7-10 by the reviewed technical
decisions in Spec Review. Kept for the record.

1. **Search-indexing source.** Session search currently indexes Pi
   transcripts via the watcher. Does "all tracking through the extension"
   include the search corpus? Options: (a) extension pushes session-notify
   with the transcript path and Quill indexes on notify (removes the
   watcher root but keeps transcript reading for indexing; ephemeral
   sessions stay unsearchable), (b) extension streams message content and
   transcripts are never read (full removal; historical/Quill-closed
   sessions need the spool), (c) indexing keeps a startup sweep plus
   notify. Decides how much of `pi_session.rs` survives.
2. **Offline coverage mechanism.** For sessions run while Quill is closed:
   extension-side durable spool with replay (where, how bounded, retention,
   at-least-once + dedupe), vs. accepting an explicit gap, vs. a one-shot
   transcript backfill retained only for catch-up. Spool-and-replay is the
   drafted recommendation; needs human confirmation because it defines the
   reliability story and the amount of extension-side state.
3. **Pre-extension history and mixed fleets.** A user upgrading Quill but
   running old Pi sessions (or a session started before upgrade completes)
   produces transcripts no event stream covers. One-time catch-up scan at
   upgrade, or explicit gap?
4. **Cost display.** Pi now supplies cost per message. Store-only, or
   surface in usage UI (which reopens the "no pricing table" stance for
   other providers)?
5. **Ephemeral session surfacing.** Track as normal live sessions
   (recommended, explicitly badged), live-only with no persistence, or
   opt-out?
6. **Turn-level analytics scope.** `transcript_analytics.rs` currently
   rejects Pi entirely. Does this feature bring Pi into runtime/turn
   analytics using turn events, or is that a follow-up epic?
7. **Endpoint shape.** Extend existing `hooks/observed` + `tokens` +
   `sessions/notify` with Pi fields, or add a dedicated versioned
   `/api/v1/pi/track` (or generic `/api/v1/agent-events`) endpoint that
   other extension-based providers could later share?
8. **npm coexistence policy.** When both the managed drop and an npm
   install are present, which wins, and how does the orphan sweep treat the
   npm copy (it is not Quill-owned by marker)?
9. **Min Pi version bump.** Which minimum Pi version do the required
   events/fields impose (e.g. `session_tree` and `targetSessionFile`
   availability), and does detection reject or degrade below it?
10. **Subagent semantics.** Core Pi has no subagents; lineage-linked
    concurrent sessions are today's proxy. Is that still the only
    "agent count" Pi surfaces, and should bash-spawned child pi processes
    (identifiable via `PI_SESSION_ID` env inheritance) ever link?

## Spec Review

Six parallel review passes (requirements, gaps, ambiguity, feasibility,
scope, stakeholders) against the constitution and the codebase. Feasibility
verified every claimed Pi API capability against installed Pi 0.84.2
(`session_start` reasons + `previousSessionFile`, `session_shutdown` +
`targetSessionFile`, `agent_settled`, turn/message/tool_execution events,
`Usage` with per-field cost, session-manager identity, `appendEntry`) — all
present, and all below the existing 0.84.0 minimum version.

### Critical Questions (answer before planning)

1. **Removal scope vs. search indexing (Open Question 1).** Sessions-view
   totals, search, lineage navigation, and Models analytics all sit on
   transcript-derived rows today; only the live fold, watcher root, and
   model-usage arms are removable outright. Does "completely removed" mean
   (a) tracking path removed — the extension notifies with the transcript
   path over the existing `sessions/notify` seam and a deliberately kept
   narrow parser still feeds search indexing (reviewers' unanimous
   recommendation), or (b) transcripts are never read again — a much
   larger build (message-content streaming, tree-rewrite dedupe, history
   gaps)? Flagged by: scope, feasibility, ambiguity, requirements.
2. **Offline coverage mechanism (Open Questions 2/3).** For sessions run
   while Quill is closed, plus pre-upgrade history: (A) extension-side
   per-session append-only spool files that Quill itself drains on
   startup/connect (no extension replay logic; covers ephemeral sessions;
   spool holds identifiers/usage/lifecycle metadata only — never message
   bodies), (B) a startup transcript catch-up scan (nearly free if Q1=(a),
   but leaves ephemeral-while-closed as an explicit gap and retains a
   transcript usage reader for catch-up), or (C) accept an explicit gap.
   Recommendation: (A), given the production-release intent. Flagged by:
   requirements, gaps, ambiguity, scope, feasibility, stakeholders.
3. **npm packaging scope (story 5).** Keep "publishable artifact now,
   publication deferred" in this feature (matches the stated intent to
   release publicly later), or cut packaging + coexistence policy to a
   follow-up spec and keep only the config-contract fix? Scope review
   recommends deferring; the problem statement leans keep. Flagged by:
   scope, stakeholders.
4. **Cost display (Open Question 4).** Pi supplies per-message cost.
   Store-only (recommended — display reopens the deliberate cross-provider
   "no pricing table" stance and is its own design task), or surface in UI
   now? Flagged by: scope, stakeholders.
5. **Ephemeral session surfacing (Open Question 5).** Recommended: live
   badged rows that vanish at shutdown — never persisted, indexed, or
   searchable. Confirm, or persist badged history rows instead? Flagged
   by: ambiguity, scope, requirements.
6. **Turn-level analytics (Open Question 6).** Bring Pi into
   `transcript_analytics` runtime/turn analytics now, or defer to a
   follow-up epic (recommended; turn events feeding the existing
   hooks/activity breakdown ship regardless)? Flagged by: scope.

### Technical Decisions (self-resolved — veto at the gate to override)

- **Dedupe key**: `responseId` is optional in Pi's types, so the extension
  mints a UUID per pushed event; the server enforces a unique index with
  insert-or-ignore. This requires a schema migration — authorized here,
  superseding spec 026's no-migration note. Pushed usage becomes the
  single Pi source for both Sessions totals and Models analytics.
- **Cut-over, not parallel run**: parity = scripted comparison of token
  totals on a real-session fixture corpus (linear, fork, resume, compact)
  run pre-merge; single-release cut-over; no dual-write in production.
- **Crash/quiescence**: server-side last-event-age eviction reusing the
  existing provider-agnostic idle sweep; a late `session_shutdown` after
  quiescence-close is an idempotent close, never a reopen. `reload` and
  `startup` reasons are continuity on the same header id — no row
  transition, no lineage edge.
- **Identity**: resume reopens the same session file → same header id;
  fork/clone mint a new id with `parentSession` provenance (verified
  against Pi's session model). Lineage reports an explicit enum —
  root / linked(parent-id) / unresolved(reason) — resolved once per
  `session_start`, upserted idempotently; hostname is lowercased at the
  ingest boundary (live keys require it; the extension currently sends
  un-normalized `os.hostname()`).
- **Endpoint shape (Open Question 7)**: extend existing endpoints;
  event-push liveness is a new server→LiveTracker mutation path (the
  existing `hooks/observed` stays audit-only); any new route is Pi-scoped
  and versioned. Explicit non-goal: no generic multi-provider agent-events
  protocol.
- **Budgets (constitution 10)**: handler synchronous work ≤10 ms (the
  already-pinned extension budget), sends bounded at the existing 1500 ms
  local timeout; only the `session_shutdown` handler awaits its POST
  (bounded — teardown, not hot path); real-Pi load test sustains a
  1000-events/min stream for 10 min with zero turn delay and bounded
  memory, measured reproducibly.
- **Backpressure**: typed status-class handling — 401 re-reads config once
  then degrades; 429/503 keep events spooled with paced backoff; Quill's
  drain throttles under its own rate limits.
- **`config.json` contract**: one shared config-writer used by Claude,
  Codex, and Pi enables (three unlocked writers exist today); Pi enable
  provisions the full contract (fixes the dead-install bug); the file
  persists on uninstall until no provider needs it; Pi repair heals drift.
  Feature gates stay placeholder-substituted in this release; the
  config-delivered-gates / byte-identical-artifact redesign moves to the
  packaging follow-up spec (clarification 3B).
- **Lifecycle hygiene**: spool and taint-marker directories join the
  owned-artifact manifest, orphan sweep, and uninstall verification.
  Quill downgrade restores the scraping payload via old-stamp repair;
  spool hard size/age caps prevent unbounded growth under permanent
  incompatibility.
- **Health**: handshake = first POST per session at `session_start`
  carrying protocol version, extension version, minimum-Quill; "alive" =
  last-report timestamp with an explicit idle state distinct from
  never-connected; mismatch → typed integration status; `last_error`
  travels in the handshake and surfaces in the Integrations detail plus a
  bounded extension log file.
- **No-Quill installs are fully inert**: missing/invalid config ⇒ zero
  tools, zero telemetry, zero disk writes (no spool), one discoverable
  notice; the package README states exactly what is captured and that it
  goes only to a local Quill.
- **npm coexistence (Open Question 8)**: deferred with packaging to the
  follow-up spec (clarification 3B). Direction recorded for it: managed
  drop wins via an instance claim; the orphan sweep never touches
  non-marker npm copies; server dedupe collapses residual double-fire;
  old-Quill + new-npm skew degrades to a typed state without retry storms.
- **Min Pi version (Open Question 9)**: stays 0.84.0 — every required
  event exists since 0.80.4; pre-1.0 drift is handled by the handshake,
  not a version bump.
- **Subagents (Open Question 10)**: status quo — lineage-linked concurrent
  sessions only; no `PI_SESSION_ID` bash-child linking (non-goal).
- **Security/process**: the repo's security-review pass runs over the new
  ingestion endpoints before release; npm provenance/2FA is part of the
  packaging story; ingestion follows the same demo-mode gate as existing
  endpoints; constitution 9 joins the Constraints (degraded states render
  slate/amber, never red; no severity-color borrowing for badges; Pi dark
  green everywhere; Integrations keeps legacy density per DESIGN.md §6).
- **Test authorization (constitution 7)**: this spec records authorization
  for the new extension/server test suites; new lat.md test specs pair
  one-to-one with them, and every removed behavior's spec section is
  superseded in the same change.

### Non-Blocking Observations

- Story 1's "vanish cleanly" pre-answered Open Question 5 — the gate
  answer supersedes the drafted wording either way.
- Deterministic session end can still be lost on process kill; the
  awaited-shutdown-POST decision above plus idle fallback covers it.
- Docs owed beyond the package README: Quill release notes describing
  visible upgrade behavior (pre-upgrade sessions go dark until Pi
  restarts, ephemeral sessions newly appear, cost data arrives).
- Multi-machine setups: `config.json` `url` may legitimately point
  off-device (the router binds 0.0.0.0 deliberately); the public README
  must document transmission posture (constitution 11), and per-host
  attribution semantics ride the config contract.
- Pre-publish checklist for the epic: package name decision, upstream Pi
  maintainer heads-up, supported-version statement ("supports Pi ≥ X,
  tested against Y").
- Predictable day-after asks to pre-empt in Non-Goals: Pi in Limits/CPA,
  cost-display parity for other providers, Windows (amplified by npm
  users), other providers on the tracking endpoints.

## Clarifications

**Q1: What does "completely removed" mean against the search index and
retained-row surfaces?**
A: 1A — the tracking path is removed; search indexing keeps reading
transcripts through a deliberately kept narrow parser, fed by extension
`sessions/notify` pushes (transcript path + identity) instead of the
watcher root. (Reflected in Goals, story 6.)

**Q2: Offline coverage mechanism?**
A: 2A — extension-side per-session append-only spool files holding
identifiers, usage numbers, and lifecycle metadata only (never message
bodies); Quill itself drains the spool directory on startup/connect, so
the extension carries no replay logic. Covers ephemeral and Quill-closed
sessions in the extension era. Pre-upgrade sessions stay searchable via
the retained parser; their analytics remain whatever the old adapter
already ingested — any remainder is an explicit gap. (Reflected in Goals,
Non-Goals, stories 1/2/4.)

**Q3: npm packaging scope?**
A: 3B — packaging, publication, and npm-coexistence policy move to a
follow-up spec. This release keeps the managed file drop as the only
deployment vehicle and keeps the config-contract fix; the extension
source itself stays held to publishable quality. (Reflected in Goals,
Non-Goals, story 4; former story 5 replaced.)

**Q4: Cost display?**
A: 4A — store-only. Pi's per-message cost is ingested and stored; no UI
displays cost this release. (Reflected in Non-Goals, story 2.)

**Q5: Ephemeral session surfacing?**
A: 5B — persisted badged rows: ephemeral sessions are live while running
and persist as explicitly badged session rows driven by pushed events,
with no transcript anchor and no search-index presence. (Reflected in
Non-Goals, story 1.)

**Q6: Turn-level analytics?**
A: 6B — in scope: extension turn/message/tool events feed the
session-events / response-times runtime pipeline so Pi appears in the
Runtime card and per-turn latency surfaces this release, ending the
`UnsupportedProvider` exclusion. (Reflected in Goals, new story 5.)

## Architecture Approach

One rewritten `quill.ts` (still a single Quill-managed file drop —
packaging deferred per clarification 3B) becomes the sole Pi tracking
source. It fans events into purpose-built lanes instead of one generic
pipe, because each lane's validation and storage already exist
(constitution 2):

- **Lifecycle/liveness/usage/health** → NEW `POST /api/v1/pi/track` on the
  main router: a versioned envelope (protocol version, extension version,
  batched events) feeding (a) new push-mutation methods on the shared
  `LiveTracker` (session start/end/activity/model/lineage/live tokens),
  (b) the usage lanes (token snapshots + a pushed model-usage lane, with
  cost stored), and (c) extension health state. `hooks/observed` stays
  audit-only, per the reviewed decision. The route is Pi-scoped and
  versioned; no generic agent-events protocol (non-goal).
- **Runtime/turn events** → existing `POST /api/v1/sessions/messages`
  live-analytics lane (source-less `session_events` + `response_times`,
  insert-or-ignore). No new runtime ingest endpoint. The read side gains
  the source-less union branch in the completed-rollup path (story 5).
- **Search indexing** → existing `POST /api/v1/sessions/notify` with the
  transcript path + resolved identity; the indexer reads the named file
  through the deliberately kept narrow parser. The watcher's Pi root, its
  120 s retry loop, and the O(n) id scan are deleted.
- **Hook/activity audit** → existing `hooks/observed` posts, upgraded to
  the full event mapping (agent boundaries, tool execution results).
- **Offline coverage** → the extension appends failed/unsent envelopes to
  per-session append-only spool files (0600, ids/usage/lifecycle metadata
  only); Quill drains the spool directory on startup and periodically
  through the same internal ingestion functions. The spool covers BOTH
  `/pi/track` and `sessions/messages` envelopes. Event UUIDs make drain
  and live delivery overlap-safe. The extension never replays.
- **Per-session runtime evidence** (`active_runtime_secs`, agent runtime
  rate): renders explicitly unknown for Pi this release — the breakdown
  query treats source-less Pi sessions as uncovered, never 0; a
  live-source evidence branch is a filed follow-up.

Alternatives rejected: extending transcript reconciliation with a Pi
parser (contradicts push, and ephemeral sessions have no file — the
reconciliation root list stays Claude+Codex); a single monolithic
tracking endpoint (duplicates validation the existing lanes already do);
extension-side spool replay (cross-process races; Quill-side drain
removes replay logic from the extension entirely); rollup rows for
source-less sources (would relax audited `NOT NULL`/contiguous-rowid
invariants — the union branch is far smaller).

Lineage is resolved in the extension exactly once per `session_start`
(bounded first-line header read of the parent file, mirroring the Rust
64 KiB probe) and pushed as `root` / `linked(parent-id)` /
`unresolved(reason)`; Quill never walks path chains again. Identity:
resume keeps the header id; fork/clone mint a new id with provenance;
`reload`/`startup` are continuity. Ephemeral sessions (no file) push the
same lifecycle with an ephemeral flag and persist as badged rows
(clarification 5B) with no search presence.

Constitution check: 1 (every gap explicit — thinking absence, open
turns, pre-upgrade remainder, spool drops); 2 (every lane extends an
existing seam); 3 (≤10 ms sync handler budget, 1500 ms bounded sends,
only shutdown awaited; drain and ingestion off UI threads); 4
(transactional install unchanged; spool append-only + drain idempotent);
5 (typed failure classes replace `catch {}`); 7 (this spec authorizes the
new suites; one-to-one lat.md specs); 8 (lat.md sync + `lat check`); 9
(health/badge states follow DESIGN.md; slate/amber, no severity
borrowing); 10 (budgets named in Testing); 11 (extension posts only to
the configured local Quill API; spool documents its contents); 12 (gated
delivery via beads).

## Affected Components

- `src-tauri/pi-integration/quill.ts` — full rewrite: tracking handlers
  (`session_start/shutdown`, `agent_start/agent_settled`,
  `turn_start/turn_end`, `message_end`, `tool_execution_start/end`,
  `model_select`, `input`), spool append, handshake, typed degradation,
  stable event/message id minting; existing tools + routing preserved.
  The three feature-gate placeholders and byte-exact substitution survive
  the rewrite (config-delivered gates deferred with packaging); the
  no-Quill notice (one discoverable message, then fully inert) and a
  bounded extension log file are part of the payload.
- `src-tauri/pi-integration/quill.test.mjs` — rewritten suite (see
  Testing Strategy).
- `src-tauri/src/server.rs` — new `/api/v1/pi/track` route (auth, rate
  limit, validation, demo gate); `sessions/messages` Pi mapping checks;
  `sessions/notify` accepts the Pi root; hostname lowercasing at ingest.
- `src-tauri/src/live_tracker.rs` — new push-mutation methods; DELETE the
  Pi fold (`fold_pi_line`, `pi_session_file`), `(mtime_ns, len)`
  fingerprint machinery, `replaced_session` eviction, and the Pi arm of
  the tail path; lineage overlay consumes pushed parent ids.
- `src-tauri/src/transcript_watcher.rs` — remove Pi root registration,
  retry loop, and ambiguous-root Pi arm.
- `src-tauri/src/model_usage.rs` + `transcript_identity.rs` — remove the
  Pi transcript adapter and active-branch turn marking; add the pushed
  usage lane (append-only, event-UUID keyed, cost columns).
- `src-tauri/src/pi_session.rs` — trim to the indexing keep-list: header
  probe + v2/v3 entry parse for `extract_pi_messages`; DELETE
  `resolve_pi_parent_session_id` fold-time use and `build_active_path`
  usage attribution.
- `src-tauri/src/sessions.rs` — indexing driven by notify (pushed path +
  identity + parent id); DELETE `find_pi_session_path_in` O(n) scan;
  ephemeral exclusion from indexing is structural (no file, no notify).
- `src-tauri/src/storage.rs` — migrations (Data Model); runtime card
  source-less union branch in the completed-rollup read (provider-agnostic
  source-less, NOT Pi-filtered — it must also surface remote-session live
  rows); Pi branch of the 600 s response cap; ephemeral flag in breakdown
  query; per-session runtime evidence explicitly uncovered for source-less
  Pi rows; token-scope `active-branch` special case removed; spool-drain
  ingestion entry.
- `src-tauri/src/transcript_analytics.rs` — the cosmetic
  `UnsupportedProvider` Pi arm is removed in the same sweep (the
  reconciliation root list stays Claude+Codex).
- `src-tauri/src/integrations/pi.rs` + `manager.rs` — config-contract
  provisioning on enable, repair drift-healing (upgrade-in-place: an
  old-stamp install deploys the new payload on startup repair),
  spool/marker/log artifacts in the owned manifest + uninstall
  verification, `config.json` persists on uninstall while any provider
  still needs it, consent copy update; the deployment stamp hashes the
  new payload bytes.
- New shared config writer (extracted from `claude_setup.rs` /
  `integrations/codex.rs` duplicated logic) used by all three providers.
- `src-tauri/src/data_paths.rs` — Pi session root used only by indexing
  path validation; placeholder-root fallback removed.
- Frontend: `IntegrationsTab.tsx` extension-health status line;
  `UsageView.tsx`/session rows ephemeral badge; `format.ts` removes the
  Pi live-token `—` special case; types for new fields.
- `lat.md/` — architecture/data-flow/backend/features rewrites; the ten
  Pi test-spec files superseded or rewritten one-to-one with the new
  suites.

## Data Model

- Migration: pushed-usage dedupe — concretely: `model_usage_observations`
  gains `event_uuid` and cost columns with a partial unique index
  (`WHERE provider='pi' AND event_uuid IS NOT NULL`) and insert-or-ignore;
  pushed rows hang off a synthetic `pi-push:<session-id>` source row
  exempt from raw-prune, and Models reads union them through the existing
  observation queries (supersedes 026's no-migration note). Cost is
  display-none this release. Read-side coexistence: legacy
  transcript-derived Pi rows (pre-cut-over) union with pushed rows;
  overlap is impossible by construction (the adapter is removed in the
  same release and pushed rows cover only post-upgrade messages), asserted
  by a resumed-across-upgrade parity fixture.
- Migration: `live_analytics_sessions` gains an `ephemeral` flag (badged
  rows) — written by the new `/pi/track` lifecycle upsert (a new writer
  added with the endpoint); breakdown query joins it.
- Extension health state: `pi_extension.*` settings rows (last-seen
  timestamp, protocol/extension version, last typed error) — no new
  table.
- Pushed model-usage lane identity: `(provider='pi', session id, event
  UUID)`; append-only, no replacement semantics; Sessions totals lose the
  `active-branch` scope label (pushed usage is spend-truth).
- Spool (on disk, not DB): `<quill-config>/pi-spool/<session-id>.<pid>.jsonl`,
  0600 files in a 0700 directory the extension creates lazily on first
  failed send (no-config installs never create it); append-only, size/age
  caps enforced by both writer and drain; drain deletes only files whose
  pid is dead or that it rename-claims before ingest — live-pid files are
  read but retained (event UUIDs make overlap safe); corrupt lines
  skipped with a surfaced typed gap.
- Extension log file: bounded (size-capped, rotating single file) beside
  the spool; part of the owned-artifact manifest and uninstall sweep.
- No changes to rollup tables or their invariants; the runtime-card union
  reads live raw rows over the same window.

## API / Interface Changes

- NEW `POST /api/v1/pi/track` (main router, bearer auth, demo-gated):
  versioned envelope
  `{protocol, extension_version, min_quill_version, events[]}` covering
  handshake, session lifecycle (+ephemeral flag, lineage enum), model
  select, usage (+cost), and health. Own rate-limiter deque, sized so
  batched envelopes carry the 1000-events/min load-test stream with ≥4×
  headroom. 400 on protocol mismatch with a typed reason the extension
  surfaces; 401/429/503 semantics per the backpressure table.
- `POST /api/v1/sessions/messages` — Pi payloads (existing shape; Pi
  supplies stable message ids, contiguous ordinals, shared message
  timestamps; sidechain identity rules apply if ever used). Pi traffic
  gets its own limiter (or a raised cap) — today's 100/min window is
  shared with `sessions/notify` and must not let Pi starve remote Codex
  sync; the load test must pass without 429s.
- `POST /api/v1/sessions/notify` — Pi provider with path validation
  against the configured Pi session root.
- `POST /api/v1/hooks/observed` — unchanged shape; fuller Pi event
  mapping.
- IPC: integration status payload gains Pi extension health; session
  breakdown payload gains the ephemeral badge field; no breaking changes
  to existing consumers (additive fields only).
- Removed behavior: live Pi token `—` gap (real numbers now);
  `tokenScope: "active-branch"` for Pi.

## Testing Strategy

Authorized by this spec (constitution 7); every new suite gets a
one-to-one lat.md spec section, and each superseded scraping spec is
rewritten or retired in the same change (constitution 8).

- Extension suite (`quill.test.mjs` successor): registration; every
  tracking handler against a loopback probe server; spool append on
  failure, caps, 0600 modes, no-config inertness (zero disk writes, one
  discoverable notice); regression for the existing eight context tools,
  routing, and routing telemetry through the rewrite; handshake +
  protocol-mismatch degradation; typed failure classes; ≤10 ms
  synchronous handler budget; real-Pi load test — 1000 events/min for
  10 min, zero turn delay, bounded RSS (constitution 10, measured).
- Rust unit tests per lane: `/pi/track` validation (auth, rate limit,
  hostname lowercasing, protocol mismatch, ephemeral flag, lineage enum
  idempotent upsert, late-shutdown idempotent close, reload/startup
  continuity); LiveTracker push mutations (start/end/replacement flows,
  crash quiescence via last-event-age eviction); pushed usage dedupe
  (replay, spool-drain overlap); runtime union (card totals correct in
  both backfill states, long-turn cap, tool-wait window, remote-session
  live rows visible post-backfill, crash mid-turn leaves no phantom
  `response_times` row, per-session runtime evidence never a false 0);
  extension health-state machine (never-connected/alive/idle/stale
  transitions, protocol-mismatch typed status, `last_error` surfacing);
  ephemeral persistence + badge; notify-driven indexing (no watcher
  root); config contract (three-provider shared writer, dead-install
  regression, file persists on Pi uninstall while Claude/Codex enabled,
  removed with the last provider); upgrade-in-place (old-stamp install →
  startup repair deploys the new payload); uninstall sweeps
  spool/markers/log.
- Parity gate (pre-merge, scripted): fixture corpus of real Pi sessions
  (linear, fork, resume, compact, resumed-across-upgrade) — pushed usage
  totals equal the old adapter's all-branch totals on linear sessions
  exactly; divergences on branched sessions are explained by
  construction, not unexplained; the resumed-across-upgrade fixture
  asserts no double count under the legacy/pushed union rule.
- E2E: installed Pi loads the rewritten extension in an isolated session
  against probe servers (successor of the existing real-Pi test); full
  cycle session → live row → usage → runtime rows → shutdown → drain
  after simulated Quill downtime.
- Removal regression: no Pi arm remains in watcher/live-fold/model-usage
  (compile-time absence + grep-clean sweep pinned by a test where
  practical); `lat check` passes with zero orphaned refs.
- Security review pass over the new/extended endpoints before release
  (repo skill), per the reviewed decision.

## Risks

- **Pi pre-1.0 API drift** — mitigations: versioned handshake, typed
  degradation, min-version 0.84.0 retained, real-Pi E2E in CI catches
  breakage at upgrade time.
- **Silent tracking loss post-removal** (extension dies → no Pi data at
  all) — mitigations: health surface distinguishes never-connected /
  idle / stale; typed last_error; hooks-breakdown attribution continues
  independently.
- **Spool edge cases** (disk full, corrupt lines, crash mid-append,
  concurrent drain) — mitigations: newline-commit appends, skip+surface
  corrupt lines, caps with explicit drop gaps, drain idempotence via
  event UUIDs; dedicated tests.
- **Runtime union query cost** — the union branch must respect the
  existing rollup read budgets (burst fold ≤10 % p95 per
  `lat.md/runtime-rollup-tests.md`); measured before merge
  (constitution 10). Unbounded live-row growth: volume estimate at Pi's
  event rate recorded in the epic; retention extension filed as
  follow-up if the estimate exceeds bounds.
- **Removal breaking retained surfaces** — mitigations: parity gate,
  keep-list defined function-by-function, staged sequencing (new lanes
  land and verify before deletion tasks unblock).
- **Concurrency** — `/pi/track` and drain take the same discipline as
  existing ingestion (rollup write permit if rows become
  rollup-visible; mutation guard untouched for installs).
- **Rollback** — a Quill downgrade's stamp repair reinstalls the old
  scraping payload; spool caps bound orphaned state; migrations are
  additive so old code ignores new columns. Re-upgrade after a downgrade
  window: adapter rows ingested during the window can overlap pushed
  rows, so the union rule excludes adapter rows for Pi sessions that have
  pushed rows (else the overlap is a documented explicit gap in release
  notes).

## Sequencing

Ordered work items; edges become the bead DAG (deps in parentheses).

1. **Server tracking foundations** — `/pi/track` endpoint (auth, own
   rate limiter, validation, demo gate), the `live_analytics_sessions`
   lifecycle upsert (ephemeral flag writer), LiveTracker push mutations,
   health settings, hostname normalization. (No deps; blocks 2, 4, 7, 8.)
2. **Extension rewrite** — tracking handlers, handshake, typed failures,
   stable id minting, spool append, no-Quill notice, bounded log, gate
   placeholders preserved, tools/routing regression; extension suite.
   (Depends only on item 1's frozen envelope schema — may start once the
   contract is committed, developed against the loopback probe server;
   blocks 8, 10.)
3. **Config contract** — shared writer, Pi enable provisioning,
   repair drift-healing, dead-install regression test, persist-until-last-
   provider behavior, consent copy. (No deps; blocks 10.)
4. **Usage lanes** — pushed token/model-usage lane (`event_uuid` + cost
   migration, `pi-push` source rows), live cumulative tokens, `—` gap and
   active-branch label removal, legacy/pushed union rule. (Depends 1;
   blocks 9, 10.)
5. **Runtime lane** — sessions/messages Pi mapping + dedicated limiter,
   response-cap Pi branch, runtime-card source-less union in both
   backfill states (provider-agnostic), runtime-evidence
   explicitly-unknown handling, measured query budget. (No deps; blocks
   9, 10.)
6. **Indexing notify lane** — notify-driven Pi indexing, parser
   keep-list trim, O(n) scan removal, pushed parent-id in search docs.
   (No deps; blocks 9, 10.)
7. **Ephemeral persistence** — badged rows over the item-1 upsert,
   breakdown/UI badge, no-index guarantee. (Depends 1, 4.)
8. **Lineage + health surfaces** — pushed lineage overlay swap,
   Integrations health status line (health enum in the IPC status
   payload, Rust-tested; UI states verified against DESIGN.md).
   (Depends 1, 2.)
9. **Spool drain** — Quill-side drain (startup + periodic), caps,
   dead-pid/rename-claim deletion, idempotent overlap with live
   delivery, downtime E2E. (Depends 4, 5, 6.)
10. **Scraping removal + parity gate** — parity fixture corpus run
    (incl. resumed-across-upgrade); delete live-fold/fingerprint/
    watcher-root/adapter/scan and the cosmetic `UnsupportedProvider`
    arm; lat.md spec supersede sweep; removal regression.
    (Depends 2, 3, 4, 5, 6, 8, 9; blocks 11.)
11. **Release hardening** — security-review pass over new endpoints,
    docs/release notes, full-gate run (`lat check`, fmt, clippy, tests,
    build). (Depends all.)

## Backlog Refinement

None. No backlog inputs exist (empty closure verified at draft and
re-verified before materialization); nothing to refine, supersede, or
retire.

## Alignment fixes applied

- Sequencing DAG corrected (B, must): spool drain now depends on the
  runtime lane; removal now depends on the lineage swap and spool drain;
  items 5 and 6 unserialized to roots; item 2 depends only on the frozen
  envelope schema.
- Per-session runtime evidence decided in-plan (A+B, must): explicitly
  unknown for Pi this release, never a false 0; live-source branch is a
  filed follow-up; test added.
- Pushed-usage migration named concretely (B, must):
  `model_usage_observations` + `event_uuid`/cost columns, partial unique
  index, `pi-push` synthetic source rows exempt from raw-prune, union
  read path; legacy/pushed coexistence rule + resumed-across-upgrade
  parity fixture (B, should).
- Ephemeral flag writer named (B, must): `/pi/track` lifecycle upsert
  into `live_analytics_sessions`, scoped into sequencing item 1.
- Rate limits pinned (B, must): `/pi/track` gets its own limiter sized
  4× the load-test stream; Pi `sessions/messages` traffic separated from
  the shared 100/min notify window.
- Spool hardening (B, should): lazy 0700 dir creation on first failed
  send, dead-pid/rename-claim deletion so live appenders never lose
  events.
- Downgrade→re-upgrade double-count rule added to Risks (B, should).
- Extension payload completeness (A, should): gate placeholders survive
  the rewrite with stamp coverage; no-Quill notice and bounded log file
  added to payload, owned manifest, uninstall sweep, and tests; README
  ships with the packaging follow-up spec.
- Test coverage added (A, should): tools/routing regression through the
  rewrite, upgrade-in-place repair, config persist-until-last-provider,
  health-state machine transitions, remote-session live rows visible
  post-backfill, crash mid-turn phantom-row guard.
- `min_quill_version` added to the handshake envelope (A, should);
  `transcript_analytics.rs` `UnsupportedProvider` arm disposition
  recorded (A, borderline).
