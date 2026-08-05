# Spec: live-subagent-count

## Problem Statement

Quill's Sessions breakdown cannot currently show how many root-linked
subagent lifecycles Quill has observed open beneath each Codex or Claude Code
session. Its existing
`has_subagents` and `subagent_count` fields describe historical retained
analytics, are not rendered, and cannot truthfully represent current observed
lifecycle state. Operators need one glanceable count whose best-effort hook
boundary is explicit rather than presented as verified process liveness.

## Goals

- Show the number of root-linked subagent lifecycles Quill has observed open
  beneath each main Codex or Claude Code Sessions row.
- Exclude the main session naturally and remove an agent after its observed
  terminal stop.
- Represent coverage explicitly in the typed contract: unknown, observed zero,
  and observed positive must remain distinct.
- Reuse Quill's existing scripts, endpoint, invalidation, audit storage, and
  frontend row grammar while keeping current-boot count state in bounded
  memory, without a new table, dependency, or UI component.
- Remove the unused historical agent fields and their four-source distinct
  count from the Sessions query.
- Preserve current query responsiveness, including the existing 300 ms
  session-breakdown performance budget.
- Keep implementation, tests, and lat.md synchronized with constitution
  principles 1, 2, 3, 5, 6, 8, 9, and 10.

## Non-Goals

- Counting Claude Code agent teams, Agent View sessions, background shell
  commands, or other top-level sessions.
- Distinguishing direct children from nested descendants; provider lifecycle
  payloads do not expose immediate-parent identity, so all root-linked
  subagents share one observed count.
- Restoring or displaying historical agent fan-out.
- Adding a subagent drilldown, header total, badge, icon, animation, new color,
  tooltip system, database table, migration, or dependency.
- Renaming the existing recency-based `LIVE` summary or changing green activity
  dots; that design debt is separate from this feature.
- Claiming verified process liveness. With normal hook delivery the count is
  exact, but an undetectable missed or blocked stop may remain observed-open
  until the parent ends or Quill restarts.
- Persisting or reconstructing positive count state across Quill restarts.

## Backlog Inputs

None. Closed investigations `quill-kbb` and `quill-pwu` are approved source
context, not backlog items requiring disposition.

## Target Epic

This run will create a new feature epic.

## User Stories

### See observed-open subagents per session

As a developer orchestrating coding agents, I want each Sessions row to show
its observed-open subagent count so that I can see parallel work at a glance.

Acceptance criteria:

- A session with three trustworthy open root-linked subagent lifecycles renders
  plain neutral `+3` immediately beside its project name.
- The main session is excluded from the count.
- A positive count does not depend on the existing five-minute `row.live`
  heuristic.
- The visible count uses tabular figures and has accessible text equivalent to
  `3 subagents observed open`.
- Long project names ellipsize before the count, provider, token, or recency
  columns clip at the documented 320 px minimum.

### Avoid false runtime claims

As an operator who trusts Quill's local numbers, I want missing lifecycle
coverage to remain unknown so that history or missed events never masquerade as
current truth.

Acceptance criteria:

- Rust exposes `observed_subagent_count: Option<u32>` and TypeScript exposes a
  required `observed_subagent_count: number | null`.
- `null` means trustworthy coverage is unavailable, `0` means covered with no
  open root-linked subagents, and a positive value means covered with that many
  hook-observed open root-linked subagents.
- UI omits both zero and null; absence makes no numeric claim.
- Events before the current Quill process's trustworthy lifecycle epoch cannot
  create a positive count.
- `SessionEnd` clears a session. `SessionStart` compaction does not reset it.
- Disabled activity tracking, unavailable hooks, or Quill restart produces
  null rather than reconstructing a positive from audit history.
- An undetectable missed or blocked stop is a documented best-effort boundary:
  it may remain observed-open until the parent ends or Quill restarts.

### Cover Codex and Claude Code consistently

As a user running both supported providers, I want equivalent root-linked
observed-subagent semantics so provider choice does not change what the count
means.

Acceptance criteria:

- Codex continues using its installed generic hook observer for
  `SessionStart`, `SubagentStart`, `SubagentStop`, and `SessionEnd`.
- Claude Code registers the same lifecycle events through its existing observer
  script under the existing activity-tracking gate.
- Lifecycle identity includes provider, hostname, root session ID, and agent
  ID, preventing one provider or host from affecting another.
- Duplicate events do not inflate counts; latest producer time wins and stop
  wins exact timestamp ties.
- Same-millisecond sibling starts remain distinct because in-memory state keys
  each root-linked agent ID independently.

### Remove dead historical work

As a maintainer, I want the deleted analytics-tree fields removed from the
Sessions query so that the new feature does not preserve unused complexity.

Acceptance criteria:

- `has_subagents` and historical `subagent_count` leave Rust and TypeScript
  `SessionBreakdown`, fixtures, query mapping, retention UI commentary, and
  their dedicated query assertions.
- The four-source distinct-agent enrichment is absent from the production
  Sessions SQL.
- Historical transcript and analytics storage remain intact; only the unused
  Sessions projection is removed.
- Relevant backend, data-flow, frontend, feature, and test-spec lat.md sections
  describe the new nullable live contract and no longer claim historical count
  rendering.

## Constraints

- Local lifecycle evidence is authoritative; unknown gaps stay explicit under
  constitution principle 1.
- Use existing Rust/Tauri storage and IPC layers plus strict TypeScript/React
  feature layers under principle 2.
- No heavy or blocking work may enter Tauri setup or the UI thread under
  principle 3.
- Expected coverage failures remain typed and display-safe under principle 5.
- Applicable lint, typecheck, build, formatting, latency, and existing tests
  must pass under principles 6 and 10.
- The user explicitly authorized the minimal automated regression tests needed
  for lifecycle folding, provider hook registration/payloads, nullable IPC,
  collision handling, and Sessions-row rendering under principle 7.
- Behavior, architecture, and test-spec changes require lat.md updates and
  `lat check` under principle 8.
- The count follows Glass Cockpit rules under principle 9: neutral color,
  stable numerics, accessible text, and no decorative chrome.
- Reuse `/api/v1/hooks/observed`, existing observer scripts, query invalidation,
  `RowModel.meta`, and `.wg-row-meta`; retain `hook_invocations` as audit history
  only and never reconstruct current count state from it.
- Count all root-linked subagents. Immediate ancestry is unavailable and is not
  inferred. Unsupported worker modes remain explicit non-goals.
- Do not bundle the separate `LIVE`/green activity cleanup.

## Open Questions

No product decisions remain open. Planning must verify provider
`SessionStart.source` schemas and choose only evidence-backed coverage epochs;
`compact` preserves state, while any source that cannot prove complete
current-boot coverage leaves the count null.

## Spec Review

### Critical Questions (answer before planning)

1. **What must a positive count mean?** Hooks prove that Quill observed a
   `SubagentStart` without a later observed `SubagentStop`; they cannot prove a
   child process is still running after a dropped hook, blocked stop, or crash.
   Choose one:
   - **A (recommended):** Define the value as hook-observed open subagent
     lifecycles. It is exact during normal delivery, becomes unknown across
     Quill restart/coverage loss, and documents that an undetectable missed
     stop can remain temporarily stale until the parent ends or Quill restarts.
   - **B:** Require verified runtime liveness, expanding scope to durable
     delivery and/or process/heartbeat reconciliation before showing a value.
   Flagged by: requirements, gaps, ambiguity, feasibility, scope.

2. **Which descendants should count?** Current Codex and Claude Code lifecycle
   payloads expose root `session_id` plus child `agent_id`, but no immediate
   parent-agent ID, so direct versus nested ancestry cannot be proven.
   - **A (recommended):** Count every hook-observed subagent linked to the
     displayed root session and describe the field as observed subagents.
   - **B:** Preserve literal direct-only scope and return null whenever direct
     ancestry cannot be proven, which may suppress both providers in v1.
   Flagged by: requirements, gaps, ambiguity, feasibility, scope.

3. **Where should current-boot state live?**
   - **A (recommended):** Maintain the fold in bounded in-memory Rust state;
     keep `hook_invocations` as audit history only. Restart naturally returns
     unknown, and Sessions avoids scanning the unbounded hook table.
   - **B:** Derive state from SQLite, accepting a schema/index migration,
     retention boundary, and additional performance work.
   Flagged by: feasibility, requirements, performance.

4. **Do you authorize automated regression test code?** Constitution principle
   7 requires explicit authorization.
   - **A (recommended):** Authorize the smallest tests needed for lifecycle
     folding, provider hook registration/payloads, nullable IPC semantics,
     collision handling, and Sessions-row rendering.
   - **B:** Add no test code; run existing validation and manual checks only.
   Flagged by: all six dimensions.

### Non-Blocking Observations

- Strict coverage remains settled: observing a child start without a
  trustworthy current-boot root epoch does not establish a partial lower bound;
  the result stays null.
- The plan must verify provider `SessionStart.source` values from current
  official schemas. `compact` preserves state; unsupported or incomplete epoch
  sources keep coverage null rather than inventing zero.
- The plan must define the full null/zero/positive truth table for startup,
  resume, clear, tracking toggles, malformed events, duplicate and
  out-of-order events, missing stops, parent end, and Quill restart.
- Reuse existing event invalidation: refresh starts within five seconds and the
  mounted row updates within six seconds including measured IPC/render time;
  add no bespoke polling or transport.
- Malformed lifecycle identity or ordering data must produce typed contextual
  diagnostics and invalidate only the affected provider/host/root coverage,
  never the whole query.
- Exact positive integers remain uncapped. Validate singular/plural accessible
  text and the 320 px layout with project label as the only shrinkable item.
- Keep process probing, TTL expiry, orphan reconciliation, persistence across
  restart, names/tasks/durations, drilldown, totals, history, and unsupported
  worker modes out of the feature and out of its bead graph.
- Preserve user-owned hook configuration through additive, idempotent,
  last-known-good integration updates; unsupported provider versions degrade to
  null without impairing existing activity tracking.

## Clarifications

**Q1: What must a positive count mean?**

A: Option A. A positive value means hook-observed open root-linked subagent
lifecycles. It is exact during normal delivery, resets to unknown across Quill
restart or lost coverage, and documents the rare missed/blocked-stop boundary.
No process or heartbeat reconciliation is added.

**Q2: Which descendants should count?**

A: Option A. Count every hook-observed subagent linked to the displayed root
session. Do not claim direct ancestry because neither provider supplies the
immediate parent-agent identity needed to prove it.

**Q3: Where should current-boot state live?**

A: Option A. Keep the fold in bounded in-memory Rust state. Preserve
`hook_invocations` as audit history only, and never reconstruct positive count
state from SQLite after restart.

**Q4: Are automated regression tests authorized?**

A: Option A. The user explicitly authorizes the smallest tests needed for
lifecycle folding, provider hook registration and payloads, nullable IPC,
collision behavior, and Sessions-row rendering.
