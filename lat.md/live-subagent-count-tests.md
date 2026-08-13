---
lat:
  require-code-mention: true
---
# Live Subagent Count Tests

These authorized tests protect transcript-derived session truth, provider delivery, nullable IPC, audit separation, and positive-only Sessions rows.

## Snapshot Reconciliation Bounds

The reconciler drops snapshots past the staleness cutoff and refuses a write older than the one it already holds for that session, so reads fail closed to null and memory stays bounded by sessions still producing evidence.

## Transcript Spawn Resolution

A tool-spawned agent is open until its spawning call has a result anywhere in the session tree, including the depth-2 case whose result lands in the parent agent's transcript rather than the root, and its metadata supplies the agent type.

## Workflow Journal Resolution

Workflow-spawned agents carry no spawning tool call, so their journal's `started` and `result` records are the only closure evidence and each agent id resolves against its own file.

## Transcript Tail Parsing

Steady-state passes consume only the bytes appended since the previous pass, and a record still mid-write is left whole rather than parsed in half, so a closure lands exactly once and only when complete.

## Transcript Idle Cutoff

An unresolved spawn whose own transcript went silent past the cutoff is an abandoned agent rather than a slow one, and a whole tree that goes quiet leaves the scan and releases its parse state.

## Transcript Session Activity

A session takes origin and project from its first record, advances activity from later timestamped content, and ignores hook-result attachments so terminal bookkeeping cannot reopen it.

## Transcript Scan Throttle

Several Sessions readers inside one interval cost a single directory walk; the throttled reads return nothing to apply and leave the reconciler holding the previous pass's state.

## Codex Rollout Turn Resolution

A Codex sub-agent is open only while its own rollout's newest turn boundary is a `task_started`, so a thread that completed or aborted its turn stops counting while still being listed with the role its head declared.

## Codex Session Activity

Codex root activity uses substantive rollout content rather than filesystem writes so terminal evidence stays authoritative until real work resumes.

### Stop Bookkeeping Filtering

Newer mtime, lifecycle, empty items, and token bookkeeping after Stop do not advance activity, while a later valid user, assistant, reasoning, or tool record does.

### Bounded Initialization and Rewrite

Initial activity reads only the bounded rollout tail, incremental scans parse appended bytes, and truncating the file clears cached activity before parsing its replacement.

## Codex Spawn Chain Grouping

Every spawn in a chain belongs to the one user thread at its root rather than to its immediate parent, across both spawn schema eras, and each agent reports the number of hops back to that root as its spawn depth.

## Codex Turn Tail Parsing

A turn's own records can push its `task_started` out of the scan window, and a window holding no boundary means the tail is still inside a turn; a record still mid-write is skipped rather than read as half a boundary.

## Codex Idle Cutoff

A rollout that died mid-turn leaves an unmatched `task_started` forever, so silence past the cutoff is the only evidence it is gone; a whole thread tree that goes quiet leaves the scan entirely.

## Codex Head Model

A Codex agent takes the first model its own rollout names, keeps that answer when a later record restates a different one, and stays unlabelled rather than borrowing a sibling's when its rollout names none or names one that fails validation.

## Observed Model Aggregation

Open agents aggregate equal validated model ids, exclude closed agents from both the count and the model lookup, retain malformed ids as unknown, and keep group totals equal to the exact open count.

## Transcript Model Stands In

Retained evidence outranks the transcript's own model wherever it reaches, and the transcript answers only for agents it has not, so a lagging ingest narrows the unknown group instead of splitting one model across two labels.

## Retained Agent Model Lookup

Claude child-model lookup selects the latest exact derived model for the requested provider, root session, and agent id without leaking evidence from another root.

## Claude Retained Model Resolution

Claude open agents resolve only from exact retained child evidence; delayed ingestion may refresh that evidence, while missing evidence stays unknown and no root model is inferred.

## Fused Merge Snapshot Consistency

A scan that swaps open membership during retained-model resolution cannot mix captured agents with model evidence from a later registry generation.

## Retained Resolution After Registry Unlock

Retained-model resolution can re-enter the observed registry, proving database work starts only after the merge guard drops.

## Sessions Terminal Evidence Projection

Storage clamps post-terminal token bookkeeping to the newest valid root Stop, StopFailure, or SessionEnd, while a strictly newer response reopens the provider-, host-, and root-matched row.

## Audit Persistence Is Non-Authoritative

Sibling hook fires sharing one timestamp retain distinct audit identities, confirming the audit table records fires rather than reconstructing open-agent membership or runtime.

## Nullable Sessions IPC Overlay

Command-layer enrichment overlays exact provider, host, and root matches while unmatched storage rows serialize active runtime and observed agents as null.

## Observed-Only Session Merge

An active root with validated root cwd synthesizes before token storage, advances a retained row's activity from the scan, and merges into that row without duplication.

## Limited Stored Session Reopening

A retained row outside the SQL limit rejoins transcript reconciliation by provider, session, and normalized host, preserving token and turn metrics even when neither stored nor scanned evidence supplies cwd.

## Observed-Only Merge Boundaries

Synthetic rows require a validated root cwd and obey normalized hostname and provider filters, selected range, deterministic limit, provider disable, and global tracking disable.

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

Retained inventory and live scanning derive a transcript's root session id through one helper per provider, so the two consumers cannot drift apart on layout rules.

Claude parents resolve to the file stem and sub-agents to the directory holding `subagents/` at any nesting depth, including the Workflow layout. Codex takes the trailing uuid of a `rollout-<timestamp>-<thread id>` name and rejects a malformed one outright rather than returning a truncated id.

## Live Tracker Tail Mechanics

A fold consumes only the bytes appended since the previous one and leaves a record still mid-write unconsumed, so activity advances exactly once and only when its record is complete.

## Live Tracker Truncation Reset

A transcript shorter than the offset already consumed was rewritten rather than appended to, so the replacement is folded whole instead of being read from a stale offset into the middle of a record.

## Live Tracker Idle Eviction

A session quiet past the idle cutoff releases both its folded state and the file offsets it owned, so memory stays bounded by live sessions and a revival re-reads its transcripts from zero.

## Live Tracker Enable Toggles

Disabling activity tracking or one provider clears the folded state it covers and keeps later folds out, and re-enabling rebuilds from the transcripts on the next sweep rather than from anything retained.
