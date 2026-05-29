# Feature Specification: Hooks Breakdown Tab

**Feature Branch**: `009-hooks-breakdown-tab`
**Created**: 2026-05-22
**Status**: Draft
**Input**: User description: "we want to add a Hooks tab to our breakdown that would show hook usage counts similarly to how we show skill usage. is this something we could reliably show given the data we have?"

## Overview

Extend the Now tab analytics breakdown with a Hooks selector — a fifth breakdown
mode alongside Sessions, Projects, Hosts, and Skills — that reports how often
lifecycle hooks fire across Claude Code and Codex sessions. Hook usage is a
first-class observability axis for users who run telemetry hooks (Quill's own
session-sync, context-capture, observe), plugin-provided hooks (such as
superpowers skill loaders), and personal hooks (commit validators, format
guards). Today users have no way to see which hooks are actually firing, how
often, against which projects, or how much overhead Quill's own telemetry
contributes.

The feasibility research that motivated this spec found that data for the two
providers comes from different sources with different granularity, and the
spec accepts that asymmetry rather than papering over it.

## Clarifications

### Session 2026-05-22

- Q: How should sub-agent hook fires appear in the Hooks breakdown? → A: Roll up into the parent script row; sub-agent identity columns (`is_sidechain`, `agent_id`) are stored on each hook record but not surfaced in the UI.
- Q: How should the Claude hook `command` field be canonicalized to form the Hook Identity? → A: Quill-aware basename normalization — Quill-managed paths collapse to `quill:<basename>`, `${CLAUDE_PLUGIN_ROOT}/<dir>/<file>` is kept verbatim, any other absolute path normalizes to its basename, and a missing `command` falls back to `hookName`.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - See hook firings at a glance (Priority: P1)

A Quill user opens the Analytics view, switches the Now tab breakdown selector
to **Hooks**, and immediately sees a list of hooks that have fired during the
selected timeframe, sorted by fire count descending. Each row shows the hook
identity, total fires, and the timestamp of the most recent fire. The view
mirrors the existing Skills breakdown in layout and interaction so users
already familiar with Skills require no relearning.

**Why this priority**: This is the entire feature's reason to exist. Without
visibility into hook activity the user has no way to answer "are my hooks
working?", "which hooks dominate session overhead?", or "is that hook I added
last week actually firing?".

**Independent Test**: With Quill running for a real Claude Code session that
includes at least one user prompt and one tool use, the Hooks breakdown lists
at minimum `SessionStart`, `UserPromptSubmit`, and a `PreToolUse:*` row (or
the matching scripts behind them, depending on provider). Counts increment as
additional events fire.

**Acceptance Scenarios**:

1. **Given** the user is on the Now tab with the default 1h timeframe and at
   least one Claude session that fired hooks in the last hour, **When** the
   user clicks the **Hooks** breakdown tab, **Then** the panel shows one or
   more rows describing hook firings, sorted by fire count descending, with a
   relative "last fired" timestamp on each row.
2. **Given** the user has the Hooks breakdown active with no hooks fired in
   the selected timeframe, **When** the panel renders, **Then** it shows the
   same empty-state messaging pattern that the Skills breakdown uses for an
   empty timeframe.
3. **Given** a hook fires in a live session while the Hooks breakdown is
   visible, **When** the next breakdown refresh tick occurs (matching the
   existing breakdown refresh cadence used by Skills), **Then** the new fire
   is reflected without requiring a window reload.

---

### User Story 2 - Distinguish Quill telemetry from user hooks (Priority: P2)

The user can identify which hook rows are deployed by Quill itself
(telemetry overhead) versus user-installed hooks (plugins, personal
automation). Quill-managed rows retain the `quill:` identity prefix in the
row text instead of showing an additional badge.

**Why this priority**: Quill's deployed telemetry hooks fire on every
session-start, user-prompt, tool-use, and stop event. Keeping the `quill:`
prefix visible identifies those rows without adding extra row chrome or
filtering the telemetry out.

**Independent Test**: With Quill's `activity_tracking` feature enabled (so
`observe.cjs` and similar Quill-deployed hooks fire), every row whose
underlying script path resolves to `~/.config/quill/...` (Claude or Codex)
keeps the `quill:` prefix in the displayed identity. Rows for any other
script path or for plugin hooks under `${CLAUDE_PLUGIN_ROOT}/...` do not
show that prefix.

**Acceptance Scenarios**:

1. **Given** Quill's `session-sync.cjs` has fired during the selected
   timeframe, **When** the Hooks breakdown renders, **Then** the row for that
   script displays as `quill:session-sync.cjs` and is otherwise sorted in
   normal fire-count order alongside non-Quill rows.
2. **Given** a user-installed Claude plugin hook fires, **When** the
   breakdown renders, **Then** the row for that hook does not display a
   `quill:` prefix.

---

### User Story 3 - Scope by provider and by lifetime (Priority: P3)

The user can scope the Hooks breakdown by provider (All / Codex / Claude) and
toggle between the active timeframe and ALL TIME, matching the controls
already present on the Skills breakdown. Selecting the Codex chip shows
event-level rows; selecting the Claude chip shows script-level rows.

**Why this priority**: Provider scoping is muscle memory for users who already
use Skills. Replicating it on Hooks keeps the UX coherent. Lifetime toggle
matters because hook installations evolve slowly, so users often want the
broader view to confirm a hook is wired up at all.

**Independent Test**: Switching the provider filter strip between All, Codex,
and Claude changes the row set to reflect only that provider's hook records.
Activating the ALL TIME chip expands counts to include all indexed data
regardless of the Now tab's active timeframe.

**Acceptance Scenarios**:

1. **Given** the Hooks breakdown is active with the provider strip on
   **All**, **When** the user clicks **Claude**, **Then** only rows backed by
   Claude transcript data remain visible.
2. **Given** the user activates the ALL TIME chip, **When** the breakdown
   recomputes, **Then** rows reflect the entire indexed history rather than
   the active timeframe window.

---

### Edge Cases

- A hook fires but exits non-zero. The fire still counts as an execution and
  appears in the breakdown; failures are not silently dropped because users
  need to see that the hook ran.
- A hook fires hundreds of times in a session (typical for PreToolUse on a
  Bash-heavy turn). Counts must not be deduplicated by tool-use ID or any
  other surrogate — each fire is one fire.
- Codex provides per-event observation only, not per-script. When the user
  enables the Codex provider filter, the rendered rows are keyed by hook
  event (and matcher where present), not by script path. This asymmetry is
  surfaced via an inline help affordance on the breakdown header so users
  understand why Codex rows look coarser than Claude rows.
- Quill's `activity_tracking` flag is off. Hook telemetry on Codex stops
  flowing because the same flag gates the new observation script. Claude
  side continues to report from transcripts (which Quill does not generate).
  The breakdown surfaces this gracefully — Codex rows go quiet, Claude rows
  continue.
- A historical Claude transcript predates the indexing change that captures
  hook attachment records. Backfill is handled by the standard reingest flag
  pattern (analogous to Skills migration 23) so existing sessions surface
  retroactively without manual intervention.
- A hook timed out (Codex `timeout=3` etc.). The fact that it started is
  observable even if it didn't complete — the Codex observer script is the
  first thing called, so it logs the event before any timeout risk.
- A hook fires inside a Claude sub-agent run (sidechain transcript). The
  fire is recorded with `is_sidechain=1` and the sub-agent's `agent_id`, but
  rolled up into the same script row as parent-transcript fires for that
  script — users see one row per script regardless of which sidechain it
  ran in.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST add a **Hooks** option to the Now tab breakdown
  mode selector, positioned after **Skills** in the existing visual order.
- **FR-002**: System MUST display one row per distinct hook identity within
  the selected timeframe and provider scope, sorted by fire count descending
  with a stable secondary sort by hook identity for tie-breaking.
- **FR-003**: For Claude hook records, the hook identity MUST be derived
  from the script command of the hook attachment record using a
  canonicalization rule of: Quill-managed paths (those whose resolved or
  absolute form points into Quill's deployed script directories) collapse
  to `quill:<basename>`; `${CLAUDE_PLUGIN_ROOT}/<dir>/<file>` is kept
  verbatim because the unexpanded env-var prefix is the only stable
  plugin-scoped identifier the transcript provides; any other absolute
  path is reduced to its basename; and a record with no `command` falls
  back to its `hookName` (e.g., `PreToolUse:Bash`).
- **FR-004**: For Codex hook records, the hook identity MUST be the
  combination of `hook_event` and `tool_name` (e.g., `PreToolUse · Bash`,
  `SessionStart`), reflecting that Codex observation is event-scoped.
- **FR-005**: System MUST persist hook observations in a dedicated table
  separate from `skill_usages`, `session_events`, and `response_times`, so
  hook analytics can evolve independently.
- **FR-006**: System MUST keep the `quill:` prefix visible in rows backed by
  Quill-deployed scripts when the canonicalized hook identity carries the
  `quill:` prefix produced by the rule in FR-003 (Claude) or when the
  observed script command resolves into Quill's managed script directories
  (Codex).
- **FR-007**: System MUST expose a provider filter strip (All / Codex /
  Claude) on the Hooks breakdown matching the Skills filter strip's
  behavior and visual style.
- **FR-008**: System MUST expose an ALL TIME toggle on the Hooks breakdown
  matching the Skills ALL TIME chip's behavior.
- **FR-009**: System MUST update the Hooks breakdown in response to the
  same refresh events that already drive the Skills breakdown (live polling
  cadence plus push-event triggered refreshes).
- **FR-010**: System MUST capture Claude hook firings by extracting
  attachment records (`type:"attachment"` with `attachment.type` starting
  with `hook_`) during the existing session indexing extraction pass,
  alongside the established skill-usage and session-event extraction.
- **FR-011**: System MUST capture Codex hook firings by deploying a new
  Quill-managed observer script registered against every Codex hook event
  type (eight events: `PreToolUse`, `PostToolUse`, `SessionStart`,
  `UserPromptSubmit`, `Stop`, `PreCompact`, `PostCompact`,
  `PermissionRequest`) without any matcher restriction, so every fire of
  every event is observed.
- **FR-012**: System MUST accept hook observations from Codex via a
  dedicated HTTP endpoint distinct from the existing learning observations
  endpoint, validating provider, hook_event, session_id, and timestamp on
  every payload before persisting.
- **FR-013**: System MUST gate Codex hook telemetry on the same
  `activity_tracking` feature flag that gates Codex tool observation today,
  so users who have opted out of activity tracking remain opted out.
- **FR-014**: System MUST backfill historical Claude hook records on next
  boot using the reingest-pending settings flag pattern already in use for
  prior schema migrations, so existing transcripts surface in the Hooks
  breakdown without requiring users to take action.
- **FR-015**: System MUST drop hook records when their owning session is
  deleted, using the same per-session cleanup path that already deletes
  `skill_usages` rows.
- **FR-016**: System MUST not introduce a new write path on Claude for hook
  observations — Claude hook data comes solely from transcript extraction
  performed by Quill, never from Quill-deployed scripts on the Claude side.
- **FR-017**: System MUST surface, via an inline help affordance on the
  Hooks breakdown header, that Codex hook records are event-scoped while
  Claude hook records are script-scoped, so the asymmetry is discoverable.
- **FR-018**: System MUST roll up hook fires recorded under sub-agent
  sidechains into the same row as their parent-transcript siblings for
  the same script identity, while preserving `is_sidechain` and `agent_id`
  on each underlying record so future analytics can split them without a
  schema change.

### Key Entities

- **Hook Invocation**: A single recorded firing of a lifecycle hook.
  Attributes: provider (Claude or Codex), session id, hook event name (e.g.
  `SessionStart`, `PreToolUse`), hook matcher / tool name when applicable,
  canonicalized script command (Claude only, may be absent on older
  records), timestamp, working directory, host, plus the sub-agent identity
  pair (`is_sidechain`, `agent_id`) inherited from the existing transcript
  ingestion contract. One row per fire.
- **Hook Identity**: The aggregation key used to group invocations into
  breakdown rows. On Claude it is the canonicalized form of the script
  command per FR-003 (`quill:<basename>` for Quill paths, verbatim
  `${CLAUDE_PLUGIN_ROOT}/...` for plugin paths, basename for other
  absolute paths, `hookName` fallback when `command` is absent). On Codex
  it is the combination of hook event and tool name.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: When the Hooks breakdown is opened on a Claude session that
  fired at least three distinct hook scripts in the active timeframe, the
  user sees at least three rows reflecting those scripts within one
  breakdown refresh tick.
- **SC-002**: For a Codex session running with `activity_tracking` enabled,
  every triggered hook event yields a row (or increments an existing row's
  count) in the Hooks breakdown within two seconds of the event firing.
- **SC-003**: Backfill of existing Claude transcripts surfaces hook
  invocations from at least the most recent 30 days of indexed session
  history within five minutes of Quill startup following the migration.
- **SC-004**: Quill-deployed hook rows are visually distinguishable from
  user-installed rows on first inspection — a user shown the breakdown can
  identify which rows are Quill telemetry without reading row contents.
- **SC-005**: Toggling the provider filter strip between All / Codex /
  Claude updates the visible row set within one frame's worth of work; no
  perceptible lag relative to the existing Skills filter strip.
- **SC-006**: When the user opts out of `activity_tracking`, no new Codex
  hook observations are persisted from that point forward; pre-existing
  observations remain visible.

## Assumptions

- Quill continues to ingest Claude transcripts via the existing dual-emission
  pipeline. The new hook extraction is one more sibling pass alongside
  skill-usages and session-events extraction; it does not require a separate
  transcript walk.
- Codex's transcript format (rollout JSONL) will not begin recording hook
  executions natively within the planning horizon of this feature. If it
  does, the Codex side can be retired in favor of transcript extraction
  without changing the storage schema or UI.
- Third-party Codex hooks (user-installed scripts registered via
  `~/.codex/config.toml`) are intentionally out of scope for v1: the Layer 2
  observer captures the event that fires them, but not the third-party
  script identity. Codex hook rows are event-keyed precisely because
  third-party attribution is unavailable.
- The existing `activity_tracking` feature flag is the correct privacy gate.
  Users who have already opted out of Codex tool observation have signaled
  they do not want Quill to record their activity; hook telemetry is
  consistent with that signal.
- Quill-managed script paths are sufficient to identify Quill-deployed hooks:
  any hook whose script command starts with `~/.config/quill/` (Codex) or
  resolves into Quill's deployed assets on Claude side counts as Quill.
- Provider parity is not the goal. Asymmetry between Claude (script-level)
  and Codex (event-level) is acceptable so long as it is discoverable to the
  user.
