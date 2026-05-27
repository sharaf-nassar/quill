# Phase 0 Research: Hooks Breakdown Tab

This document records the design decisions taken before any code change.
Each decision is identified by an R-letter so plan.md and tasks.md can
reference them. All decisions resolve every NEEDS CLARIFICATION marker
from `spec.md` (none remain).

## R-A — Storage shape: dedicated `hook_invocations` table

**Decision**: Create a new `hook_invocations` table parallel to
`skill_usages`. Do not extend `session_events` and do not reuse
`response_times`.

**Rationale**:
- `session_events` (migration 26) carries per-message timeline rows with
  `kind ∈ {user_text, user_tool_result, asst_text, asst_thinking,
  asst_tool_use}`. Hook fires lack a corresponding message identity (no
  message UUID, no assistant/user role), so they would force an awkward
  taxonomy expansion and pollute the runtime active-interval computation
  in `get_llm_runtime_stats`.
- `skill_usages` is the closest precedent: a sibling per-event table
  populated during the same extraction pass, with its own per-cwd
  drilldown indices and per-session cleanup. The Hooks feature has the
  same access patterns (breakdown query, per-cwd filter, per-session
  delete, backfill on reingest flag).
- A dedicated table lets hook analytics evolve independently — adding an
  `exit_code` or `duration_ms` column later does not require touching the
  hot-path runtime tables.

**Alternatives considered**:
- *Extend `session_events` with a `hook` kind*: rejected because hook fires
  break the per-message identity model the runtime computation depends on.
- *Reuse `tool_actions`*: rejected because hooks are out-of-band events
  unrelated to tool invocations.
- *Single combined `usages` table for skills + hooks*: rejected because
  the two have different identity rules (skill canonicalization vs hook
  canonicalization) and merging them would force a union schema with mostly
  null columns.

## R-B — Claude ingestion: third sibling in the dual-emission extractor

**Decision**: Add `extract_hook_invocations_from_attachment` to
`src-tauri/src/sessions.rs`. Invoke it from the existing JSONL parse pass
that already produces `ExtractedMessage` (for the search index) and
`ExtractedEvent` (for `session_events`). Attachment records are inspected
per-line; the extractor returns zero or more `HookInvocationInput` rows
collected by the caller and inserted via
`store_hook_invocations_for_messages`. No second walk of the transcript is
required.

**Rationale**:
- The Sub-Agent Transcripts machinery (`agent-*.jsonl` files) already feeds
  into the same parser with `is_sidechain=1` and `agent_id` set. Putting
  the new extractor in the same pass means sub-agent hook fires inherit
  those identity columns for free, satisfying FR-018.
- The `lat.md/data-flow#Session Indexing Pipeline#Dual Emission for Runtime
  Tracking` section already documents the dual-emission contract. Adding a
  third sibling keeps the contract intact rather than introducing a new
  parallel pipeline.
- Reingest backfill via the `hook_invocation_reingest_pending` settings
  flag (see R-E) hooks into the existing post-boot sweep at
  `src-tauri/src/sessions.rs:737`, so no new background task is needed.

**Alternatives considered**:
- *Separate post-pass walker over `~/.claude/projects/`*: rejected — would
  double JSONL read I/O and complicate the indexing lifecycle.
- *Tantivy-based extraction*: rejected — attachment records are not
  currently indexed for search, and indexing them just to extract a count
  would balloon the index size.

## R-C — Codex ingestion: dedicated observer script + HTTP endpoint

**Decision**: Ship one new managed script
`src-tauri/codex-integration/scripts/hook-observe.cjs`. The installer
registers it as a `command` entry on each of the eight Codex hook events
(`PreToolUse`, `PostToolUse`, `SessionStart`, `UserPromptSubmit`, `Stop`,
`PreCompact`, `PostCompact`, `PermissionRequest`) with no matcher, so every
fire is observed. The script reads stdin, extracts the relevant fields,
POSTs `{provider, hook_event, tool_name, session_id, cwd, ts}` to a new
`POST /api/v1/hooks/observed` endpoint, and exits 0 without waiting on
indexing.

**Rationale**:
- Codex rollout JSONL does not record hook executions (feasibility
  research confirmed across three sampled sessions, ~2700 lines combined).
  Transcript-based extraction is impossible.
- A dedicated single-purpose script is preferable to extending
  `observe.cjs` because:
  - `observe.cjs` carries learning-system semantics and posts to
    `/api/v1/learning/observations`; mixing concerns would force
    server-side branching on payload shape.
  - The observer can be deployed cleanly on every event without disturbing
    the existing `observe.cjs` `tool_name === "Bash"` filter that the
    learning pipeline relies on.
- The endpoint follows the fast-ack contract already documented in
  `lat.md/backend#HTTP API Server#Endpoints`: validate, acknowledge,
  persist on a background blocking task. Codex's 5-second hook timeout
  (used by the existing managed scripts) is comfortably honored.

**Alternatives considered**:
- *Layer 1 (self-reporting in each existing Quill script)*: rejected per
  user direction during clarification — Layer 2 captures every Codex hook
  fire we care about; self-reporting adds N redundant POSTs per event.
- *Layer 3 (`listHooks` JSON-RPC enumeration of third-party hooks)*:
  rejected for v1 — out of scope, captured in Assumptions as a future
  enhancement if user demand surfaces.
- *Wrapping third-party hook commands in config.toml*: rejected because it
  invalidates Codex's `trusted_hash` mechanism and would re-prompt users
  on every Quill installer run.

## R-D — Hook identity canonicalization rule

**Decision**: Form the Hook Identity at insert time according to the rule
recorded in spec FR-003 and the Clarifications session:

1. If the script command resolves into Quill-managed script directories
   (Claude: deployed assets in `~/.config/quill/...`; Codex: scripts under
   `~/.config/quill/codex/scripts/...`), emit `quill:<basename>`.
2. Else if the command begins with `${CLAUDE_PLUGIN_ROOT}/`, keep the full
   `${CLAUDE_PLUGIN_ROOT}/<dir>/<file>` form verbatim (the unexpanded env
   var is the only stable plugin-scoped identifier the transcript
   provides).
3. Else, normalize to the basename of the executable portion of the
   command (drop quoting, leading `node`/`bash` invokers, and any trailing
   args).
4. If `command` is absent (older Claude transcripts), fall back to
   `hookName`.

The canonicalized form is the row's `hook_identity` column. The raw
`command` is preserved in a separate column so future audit views can
display the verbatim string if needed.

**Rationale**:
- Quill identity must be stable across machines so the QUILL chip behaves
  consistently — `quill:<basename>` strips machine-specific path prefixes.
- `${CLAUDE_PLUGIN_ROOT}` is preserved because two plugins with the same
  relative hook path are inherently indistinguishable in the transcript;
  collapsing them to basename would silently merge unrelated plugin
  hooks.
- Basename normalization for the third bucket keeps personal hooks
  visible without splitting on OS-specific path forms.
- Storing both `hook_identity` (canonicalized) and `script_command_raw`
  (verbatim) lets future analytics drill from the row down to the literal
  command text without requiring a schema migration.

**Alternatives considered**:
- *Hash the full command*: rejected — defeats human readability and
  prevents the QUILL chip lookup.
- *Use `hookName` as the sole identity*: rejected — different scripts on
  the same event (e.g., multiple `PreToolUse:Bash` hooks) collapse,
  hiding the answer to "which script is firing".

## R-E — Backfill: `hook_invocation_reingest_pending` settings flag

**Decision**: Migration 27 sets `hook_invocation_reingest_pending = "1"`
in the existing `settings` table. The post-boot sweep at the bottom of
`sessions.rs` reads any non-empty reingest-pending flag, walks every
indexed Claude JSONL via the existing mtime-tracked path, and replays the
new attachment-extractor across each transcript. The flag is cleared after
a clean sweep, matching the behavior of `skill_usage_reingest_pending`
(migration 23) and `runtime_event_reingest_pending` (migration 26).

**Rationale**:
- The pattern is already proven by two prior migrations. It avoids
  introducing a new background worker.
- Codex has no historical data to backfill — its hook records only exist
  prospectively via the new observer endpoint. The flag is therefore
  Claude-only in effect even though it lives in the shared settings
  table.
- Reingest is idempotent because `hook_invocations` carries a UNIQUE
  index over `(provider, session_id, timestamp, hook_identity)`.

**Alternatives considered**:
- *Lazy ingestion on first breakdown query*: rejected — first-render
  latency would balloon on machines with thousands of historical
  transcripts.
- *One-shot backfill on migration*: rejected — migration runs inside a
  short-lived txn and cannot afford long-running I/O.

## R-F — UI integration: BreakdownPanel mode + QUILL chip + filter strip

**Decision**: Extend `src/components/analytics/BreakdownPanel.tsx` with a
new `'hooks'` mode parallel to `'sessions' | 'projects' | 'hosts' | 'skills'`.
The Hooks mode renders one row per `hook_identity` with columns Hook /
Uses / Last used, sorted by Uses descending. Rows carry a QUILL chip
(parallel to the existing AGENT chip from the Sessions tab's sub-agent
disclosure) when `hook_identity` starts with `quill:`. The provider filter
strip (All / Codex / Claude) and the `∞ ALL TIME` chip are reused unchanged
from the Skills implementation. An inline help affordance (a `?` button
identical to the Now-tab insight-card pattern) on the breakdown header
explains the Claude/Codex granularity asymmetry per FR-017.

**Rationale**:
- Re-using `BreakdownPanel.tsx`'s mode-switching scaffolding means no new
  panel component is created; the change is additive.
- The chip vocabulary on this panel already includes CLAUDE / CODEX /
  AGENT chips. Adding QUILL fits the same vocabulary.
- The existing `useBreakdownData.ts` hook already implements the All-TIME
  toggle and provider scoping for skills; the new `useHookBreakdown`
  variant duplicates the smallest possible amount of code to reuse the
  state-pattern documented in
  `lat.md/frontend#Custom Hooks#State Pattern`.

**Alternatives considered**:
- *Standalone Hooks panel*: rejected — would duplicate sorting, header
  layout, ALL TIME toggle, and refresh wiring.
- *Promoting Hooks to a top-level analytics tab*: rejected — fire counts
  belong alongside the existing analytics breakdown axes (Sessions,
  Projects, Hosts, Skills), not as a peer to Now / Trends / Charts /
  Context.

## R-G — Documentation updates

**Decision**: Update `lat.md/` in lockstep with the code change. Required
edits:

- `lat.md/backend.md` — add a "Hook Invocations" subsection under the
  Schema section; add `/api/v1/hooks/observed` to the Endpoints table; add
  `get_hook_breakdown` to the Tauri IPC Commands listing.
- `lat.md/data-flow.md` — extend the Session Indexing Pipeline section to
  mention attachment-record extraction as the third sibling of message and
  event extraction.
- `lat.md/features.md` — extend the Analytics Dashboard / Now Tab section
  to describe the Hooks breakdown row, the QUILL chip, and the
  Claude/Codex asymmetry help affordance.
- `lat.md/infrastructure.md` — extend the Codex Integration Deployment
  section to note `hook-observe.cjs` and its 8-event registration; note
  the `activity_tracking` gating.
- `lat.md/tests.md` — add test-spec sections for the new extractor, the
  new endpoint, the new IPC command, and the breakdown SQL.

`lat check` must pass before completion.

**Rationale**: Project CLAUDE.md requires `lat.md/` to stay in sync with
the codebase. Each touched code area corresponds to a documented section.

## Open questions

None. All clarifications from spec.md are resolved (per Clarifications
session 2026-05-22). Retention semantics and installer-upgrade flow for
the Codex observer are explicitly deferred plan-level items captured in
the data model and the installer section respectively; both follow
existing patterns (session-lifetime deletion via
`delete_hook_invocations_for_session`; installer idempotency via the
existing managed-script removal/reinstall path).
