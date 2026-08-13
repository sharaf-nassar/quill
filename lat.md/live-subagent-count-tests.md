---
lat:
  require-code-mention: true
---
# Live Subagent Count Tests

These authorized tests protect the live fold over local transcripts, provider delivery, nullable IPC, audit separation, and positive-only Sessions rows.

## Sessions Terminal Evidence Projection

Storage clamps post-terminal token bookkeeping to the newest valid root Stop, StopFailure, or SessionEnd, while a strictly newer response reopens the provider-, host-, and root-matched row.

## Audit Persistence Is Non-Authoritative

Sibling hook fires sharing one timestamp retain distinct audit identities, confirming the audit table records fires rather than reconstructing open-agent membership or runtime.

## Nullable Sessions IPC Overlay

Command-layer enrichment overlays exact provider, host, and root matches while unmatched storage rows serialize active runtime and observed agents as null.

## Limited Stored Session Reopening

A retained row outside the SQL limit rejoins the result through the fold's own ranking keys by provider, session, and normalized host, preserving token and turn metrics even when neither the stored row nor the fold supplies cwd.

## Claude Managed Observer Hooks

Claude setup adds the observer to existing Stop, StopFailure, and SessionEnd groups only under activity tracking, without duplicate groups or subagent lifecycle handlers.

## Claude Terminal Payloads

Claude's observer sends provider, root session, host, cwd, producer time, and exact event for Stop, StopFailure, and SessionEnd while rejecting unrelated lifecycle events.

## Codex Audit Payloads

Codex payload construction preserves event, tool, root and agent identity, hostname normalization, legacy session fallbacks, and malformed-evidence safety, carrying no field the audit row does not store.

## Codex Managed Observer Hooks

Codex integration verification registers the observer on exactly eight events including SessionEnd, excludes all other lifecycle events, and removes it when activity tracking is disabled.

## Terminal Hook Liveness

The Sessions indicator treats a terminal hook tied with or newer than activity as stopped, reopens on strictly newer activity, and keeps recency fallback for missing or invalid markers.

## Live Runtime Extrapolation

Runtime extrapolation preserves unavailable values and selects precision per datum. Lifetime values floor minutes; visible current turns floor seconds into adaptive clocks, while ARIA and agent runtimes remain human-readable.

## Sessions Agent Runtime Rows

Frontend contracts keep retained agent count, bot icon, and runtime before root turns and family runtime on the main row, and accrue known totals only for runtime-known active observed agents.

Positive historical totals remain visible without adding a second row, while empty zero/unknown groups disappear. Only open agents create a wrapping second rail; unknown totals stay unknown beside them, and instant tooltips plus accessible labels distinguish lifetime totals from green live names and neutral runtimes.

## Observed-Only Sessions Presentation

Frontend provenance formatting renders unavailable synthetic tokens as an em dash and omits the false zero-turn tooltip claim while retained rows keep real metrics.

## Shared Root Session Id

Retained inventory and the live fold derive a transcript's root session id through one helper per provider, so the two consumers cannot drift apart on layout rules.

Claude parents resolve to the file stem and sub-agents to the directory holding `subagents/` at any nesting depth, including the Workflow layout. Codex takes the trailing uuid of a `rollout-<timestamp>-<thread id>` name and rejects a malformed one outright rather than returning a truncated id.

## Live Tracker Tail Mechanics

A fold consumes only the bytes appended since the previous one and leaves a record still mid-write unconsumed, so activity advances and a spawn closes exactly once, and only when the record carrying that evidence is complete.

## Live Tracker Truncation Reset

A transcript shorter than the offset already consumed was rewritten rather than appended to, so the replacement is folded whole instead of being read from a stale offset into the middle of a record.

## Live Tracker Idle Eviction

A session quiet past the idle cutoff releases both its folded state and the file offsets it owned, so memory stays bounded by live sessions and a revival re-reads its transcripts from zero.

## Live Tracker Sweep Idle Gate

A sweep skips a transcript whose file has not been written since before the idle cutoff, however recent the records inside it read, and folds the same file once a write lands back inside the window.

The gate is what keeps a sweep from re-reading the whole corpus from byte zero after eviction released its offsets, and it stops the metadata retry for sub-agents that are long gone.

## Live Tracker Enable Toggles

Disabling activity tracking or one provider clears the folded state it covers and keeps later folds out, and re-enabling rebuilds from the transcripts on the next sweep rather than from anything retained.

## Live Tracker Spawn Resolution

A tool-spawned agent stays open until its spawning call has a result anywhere in the session tree, including the depth-2 case whose result lands in the parent agent's transcript rather than the root.

The session takes its origin and project from the root transcript's first record, and each agent takes its type from the metadata written beside it.

## Live Tracker Workflow Journal

Workflow-spawned agents carry no spawning tool call, so the journal the fold pulls in beside them is their only closure evidence and each agent id resolves against its own `result` record.

## Live Tracker Abandoned Spawn

An unresolved spawn whose own transcript went silent past the cutoff is abandoned rather than slow, while a sibling still writing stays open and keeps the session itself live.

## Live Tracker Session Activity

A session takes origin and project from its first record and keeps them, advances activity from later timestamped content, and ignores hook-result attachments so terminal bookkeeping cannot reopen it.

## Live Tracker Agent Metadata

A `.meta.json` that lost the race to the transcript beside it is picked up by a later event rather than never, so the agent gains its type and its spawning call without any new transcript bytes behind the retry.

## Live Tracker Agent Model

An agent's model comes from its own assistant records and passes the same validation retained evidence does, so a malformed id leaves the agent unlabelled instead of mislabelled.

## Live Tracker Codex Turn Resolution

A Codex sub-agent counts only while its own rollout's newest turn boundary is a `task_started`, so a thread that completed or aborted its turn stops counting while still being listed with the role its head declared.

The user thread at the root of the chain is the session, never one of its own agents, and it takes its origin and project from that root rollout's head.

## Live Tracker Codex Session Activity

Codex root activity comes from substantive rollout content, so lifecycle, token bookkeeping, and empty items appended after a turn ends cannot reopen a finished session while a later valid tool record does.

## Live Tracker Codex Bounded Initialization

A rollout's first fold reads only a bounded tail, so activity older than that window falls back to the thread's own start rather than costing a read of the whole file, and later folds parse only appended bytes.

A rewritten rollout is shorter than the offset already consumed, which clears the activity it had contributed before its replacement's tail is folded.

## Live Tracker Codex Agent Model

A Codex agent takes the first model its own rollout names and keeps that answer when a later record restates a different one, and stays unlabelled rather than borrowing a sibling's when its rollout names none or names one that fails validation.

## Live Tracker Codex Spawn Chain

Every spawn in a chain folds into the one user thread at its root rather than into its immediate parent, across both spawn schema eras, so a grandchild reaching the root only by hopping through its parent joins the same session.

## Live Tracker Codex Turn Tail

A turn's own records can push its `task_started` out of the bounded window, and a window holding no boundary means the tail is still inside a turn; a boundary still mid-write is left unconsumed rather than read as half a record.

## Live Tracker Codex Idle Cutoff

A rollout that died mid-turn leaves an unmatched `task_started` forever, so silence past the cutoff is the only thing that stops it counting, and once its root goes quiet too the whole tree leaves the fold and releases its offsets.

## Live Tracker Read Overlay

A folded session storage has no row for becomes an observed-only row carrying its open agents in a stable order, and only a validated root cwd earns it: without one it names no project and stays out of the result.

A stored row the fold does not cover keeps unknown agents and its own retained metrics.

## Claude Rail Through The Read Path

A spawned Claude agent reaches a Sessions row as an open agent carrying its own transcript's model, a process that never saw the spawn rebuilds that rail from the transcripts alone, and the spawning call's result closes it.

A row for a host with no local transcripts rides the same read unchanged: its agent fields stay null rather than borrowing the local fold's answer.

## Codex Rail Through The Read Path

A spawned Codex rollout reaches a Sessions row the same way, with the model its own rollout names, survives a restart through the sweep that rebuilds the fold, and leaves the rail when its turn boundary closes.

## Read Path Without Scan On Read

A read over a folded corpus opens no transcript, so it holds the Sessions budget with headroom while the fold that produced the state is paid on the sweep instead.
