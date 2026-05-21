# Feature Specification: Active-Time Runtime Tracking Redesign

**Feature Branch**: `008-runtime-redesign`
**Created**: 2026-05-20
**Status**: Draft
**Input**: User description: "yes go ahead with the redesign and implementation after ensuring it is the best approach"

## Context

The Now tab's **LLM Runtime** card today reports a number that is one to two orders
of magnitude smaller than the time users actually spend in their CC and Codex
sessions. Investigation traced the cause to the session indexer's message
extraction filter dropping `tool_result`-only user messages and to a turn-pair
ingestion model that records only the first assistant line of each generation.
This feature redesigns how active session time is measured, ingested, and
queried so that the card matches what users observe in their day-to-day work.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Trustworthy active-time metric on the Now tab (Priority: P1)

A user opens the Quill widget after a heavy CC or Codex working session and
glances at the LLM Runtime card. They expect the displayed total to reflect
the time their assistant was actively engaged on their behalf — including
model generation, tool execution, and reasoning blocks — for the selected
window. Long pauses where the user stepped away should not inflate the total,
but long tool executions where the assistant kept working should count.

**Why this priority**: This card is the headline metric users look at when
deciding "did I really spend hours on this today?" If it understates by a
factor of 50 it loses all trust, devaluing every nearby card by association.

**Independent Test**: Open the Now tab on a development machine that has been
running CC for several hours of mixed prompting and tool execution. Compare
the card's "total" against the user's own recollection (or against the
session's transcript span minus obvious idle gaps). The displayed total
should agree to within a small margin.

**Acceptance Scenarios**:

1. **Given** a session where the user typed one prompt, the assistant ran a
   30-minute agent loop with many tool calls, and then printed a final
   answer, **When** the user views the LLM Runtime card with that window
   selected, **Then** the displayed total reflects roughly the full 30
   minutes of active work rather than only the seconds it took to print the
   first assistant block.
2. **Given** a session where the user typed a prompt, walked away for an
   hour, then returned and typed another prompt, **When** the user views
   the LLM Runtime card, **Then** the hour of idle time is NOT included in
   the total.
3. **Given** a session where a single tool call took 15 minutes (e.g., a
   long-running build or a sub-agent task), **When** the user views the
   LLM Runtime card, **Then** the 15 minutes ARE included in the total
   because the assistant was waiting on tool output, not idle.

### User Story 2 - Per-session and per-sub-agent attribution stays correct (Priority: P1)

The Now tab's Sessions breakdown lists parent CC/Codex sessions and lets the
user expand them to see the sub-agents they dispatched. Each row reports a
turn count and activity timestamps. After the redesign, those numbers must
still be correct, and parent rows must not double-count time that a
sub-agent already counted.

**Why this priority**: The session breakdown shares its underlying data with
the headline card. A redesign that fixes the headline but breaks the
breakdown is a regression.

**Independent Test**: Expand a session in the Sessions breakdown that
dispatched at least one sub-agent. Verify (a) the parent's turn count is
not zero, (b) each sub-agent row shows its own turn count and activity
range, and (c) the parent's reported total active time is consistent with
the union of its own turns and its sub-agents' turns (not their sum, since
the parent is waiting while the sub-agent runs).

**Acceptance Scenarios**:

1. **Given** a parent session that dispatched two sibling sub-agents from
   the same parent message, **When** the user expands the parent row,
   **Then** the two sibling sub-agents appear as separate rows with their
   own activity windows, and the parent's total active time does not
   double count the time it spent waiting on either sibling.
2. **Given** the existing `parent_only` filter is selected, **When** the
   user views the LLM Runtime card, **Then** sub-agent activity is
   excluded from the total, matching today's filter semantics.

### User Story 3 - Historical data backfills automatically (Priority: P2)

A user upgrades to the build that includes this feature. They have months
of CC and Codex transcripts already on disk. They expect their existing
sessions to show meaningful runtime numbers after the upgrade, not stay
stuck at the old undercounted values until they manually trigger a
reindex.

**Why this priority**: Without backfill, only new sessions benefit; users
with active history would not see the fix until weeks of new activity
diluted the old data.

**Independent Test**: On a machine with existing CC and Codex transcript
history, upgrade to the build. Within a short period of normal app use,
the LLM Runtime card for the 7-day and 30-day windows reports totals that
reflect the redesigned semantics applied to the existing transcripts.

**Acceptance Scenarios**:

1. **Given** the user upgrades the app, **When** they open the Now tab,
   **Then** historical sessions are reindexed against the new model
   without requiring a manual action.
2. **Given** reindexing is in progress for a large transcript set,
   **When** the user views the card, **Then** the card remains responsive
   and progressively updates as data lands.

### User Story 4 - The card's description matches its math (Priority: P3)

The "?" help tooltip on the LLM Runtime card describes what the metric
measures. After the redesign, that description must accurately describe
the new semantics so users do not see a number that contradicts the words
next to it.

**Why this priority**: Lower than the math itself, but trust in any
analytic depends on the explanation being honest.

**Independent Test**: Open the tooltip on the LLM Runtime card. The
description references active time including tool execution, and
explicitly notes user-idle gaps are excluded. The Sessions breakdown row
tooltips are consistent with the headline.

**Acceptance Scenarios**:

1. **Given** a user clicks the help icon, **When** the tooltip appears,
   **Then** the wording matches the implemented semantics and does not
   claim the metric measures "model generation only".

### Edge Cases

- A session contains assistant messages with no preceding user message in
  the visible window (transcript window starts mid-session). The first
  visible event starts a logical turn rather than being dropped.
- A session has clock skew where an assistant timestamp is slightly
  earlier than the user message that should precede it. Negative gaps are
  treated as zero rather than as new turns.
- A sub-agent transcript file appears before its parent has been indexed.
  Both files are still picked up and attributed correctly once both are
  on disk.
- A single user prompt produces a multi-block assistant generation
  (thinking + text + tool_use written as three JSONL lines in quick
  succession). All three contribute to the active interval; none is
  dropped as a duplicate.
- A tool call legitimately runs for hours (long build, sub-agent task).
  The gap counts as active time, capped by a safety ceiling to defend
  against stuck processes or clock errors.
- The user spans midnight or a daylight-saving-time boundary during a
  session. Window cutoffs use absolute timestamps so the math does not
  shift.
- The user is running both CC and Codex sessions concurrently. Both
  contribute, but rows from different providers do not stitch together
  into a single logical turn.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST record every non-meta `user` and `assistant`
  message from CC and Codex transcripts as a discrete timestamped event,
  preserving provider, session, sub-agent identity, and a classification
  of the event kind (real user prompt, tool result, assistant text,
  assistant thinking, assistant tool use).
- **FR-002**: The system MUST NOT discard `tool_result`-only user messages
  from the runtime data path; the search index MAY continue to omit them.
- **FR-003**: The LLM Runtime card MUST report total active time across
  the selected window, computed as the sum of per-chain logical turns
  where each turn is a contiguous run of events whose between-event gaps
  either are within an idle threshold or fall between tool-loop bracketing
  events.
- **FR-004**: A gap between an `assistant` event ending a generation
  (text or final block) and the next conversational `user_text` event
  longer than a published idle threshold MUST split the logical turn so
  that user-idle time is not counted.
- **FR-005**: A gap between a `tool_use` assistant event and its
  corresponding `user_tool_result` event MUST be counted as active time
  irrespective of length, up to a published safety ceiling that defends
  against clock skew or stuck processes.
- **FR-006**: The system MUST scope chains by `(provider, session_id,
  agent_id)` so sibling sub-agents spawned from the same parent message
  do not stitch into one timeline, matching the documented attribution
  model.
- **FR-007**: The existing `scope = "parent_only"` request to the runtime
  query MUST continue to exclude sub-agent rows so the headline card can
  represent parent-thread cost without sub-agent inflation.
- **FR-008**: The system MUST automatically backfill existing transcripts
  on first launch after upgrade, with no user action required. Backfill
  MUST be incremental and MUST NOT block the UI.
- **FR-009**: Re-ingesting a transcript MUST be idempotent: running the
  indexer twice over the same JSONL file MUST NOT change the recorded
  totals.
- **FR-010**: The card's published help text and the lat.md sections that
  describe the runtime metric MUST be updated to match the new semantics
  in the same change that ships the new math.
- **FR-011**: The Sessions breakdown and sub-agent tree views MUST
  continue to render correct per-session and per-agent activity windows
  and turn counts after the redesign, either by remaining on the existing
  data source or by being repointed at the new one — the redesign MUST
  NOT regress either view.
- **FR-012**: Deleting a session MUST clear its rows from the new event
  data source, in the same operation as the existing session-delete flow.
- **FR-013**: Window queries MUST use absolute timestamps for the lower
  bound so that the displayed total is the same regardless of the
  machine's local timezone or DST transitions during the window.

### Key Entities *(include if feature involves data)*

- **Session event**: A timestamped record of one line from a CC or Codex
  transcript, attributed to a provider, parent session, optional
  sub-agent, and classified by event kind. The fundamental unit the new
  metric is computed from.
- **Logical turn**: A contiguous run of session events on a single chain
  bounded by idle gaps that exceed the configured idle threshold (with
  the tool-loop exception). The unit summed into "total active time" and
  used to derive turn count.
- **Chain**: A unique `(provider, session_id, agent_id)` tuple — the
  parent transcript and each sub-agent each have their own chain so they
  do not stitch together.
- **Idle threshold**: The published gap length above which a gap between
  conversational events is treated as user-idle and splits a logical
  turn. The same parameter is exposed by the help tooltip.
- **Tool-wait safety ceiling**: The published upper bound on how long a
  single tool-wait gap may contribute to a turn before being clamped, to
  defend against clock errors or stuck processes.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For a representative sample of recent CC sessions on a
  developer machine, the LLM Runtime card's total for the last 1 hour
  is within ±15 percent of the wall-clock active time independently
  measured from the same transcripts using the documented definition.
- **SC-002**: On a session where the user steps away for 30 minutes
  between prompts, the LLM Runtime card excludes the 30-minute gap from
  the total.
- **SC-003**: On a session containing one tool execution longer than 15
  minutes, the LLM Runtime card includes that execution time in the
  total.
- **SC-004**: After upgrading on a machine with at least 100 historical
  sessions, the LLM Runtime card reflects the redesigned totals for at
  least the last 24 hours of history within 10 minutes of app launch,
  without the user clicking anything.
- **SC-005**: Re-running the session indexer over the same transcripts
  produces byte-identical totals on every run.
- **SC-006**: The Sessions breakdown and sub-agent tree continue to
  render activity windows and turn counts for every existing session
  without empty rows or attribution swaps.
- **SC-007**: The card's help tooltip text and the corresponding lat.md
  sections describe the new semantics and pass `lat check`.

## Assumptions

- The fix targets the Quill widget's analytics layer; provider-side
  changes to CC or Codex are out of scope.
- Transcripts on disk are the source of truth; the new model is
  re-derivable from them at any time, so no schema migration loses
  irreversible data.
- The new event table is acceptable to add even if it duplicates some
  data already held in `response_times`; the existing table can stay
  for now to back the Sessions breakdown and sub-agent tree without
  regression risk.
- The default idle threshold and tool-wait safety ceiling will be
  picked during implementation to match observed transcript patterns,
  with sensible defaults selected by the implementer and documented in
  the help tooltip.
- The Now tab's existing time-range selector (1h, 24h, 7d, 30d) covers
  the windows users care about; no new range options are introduced
  in this feature.
- Backfill of historical transcripts uses the existing session-indexer
  walk and idempotency logic, so the feature does not introduce a new
  background process or scheduler.
- Storage cost of the new event table is acceptable on developer
  machines (estimated tens of megabytes per thousand sessions, far
  below current SQLite footprints).
