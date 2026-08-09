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

A session takes its origin and project from the first record it ever parses and advances its activity from later appends, so re-reading the start record is never required to keep either.

## Transcript Scan Throttle

Several Sessions readers inside one interval cost a single directory walk; the throttled reads return nothing to apply and leave the reconciler holding the previous pass's state.

## Codex Rollout Turn Resolution

A Codex sub-agent is open only while its own rollout's newest turn boundary is a `task_started`, so a thread that completed or aborted its turn stops counting while still being listed with the role its head declared.

## Codex Spawn Chain Grouping

Every spawn in a chain belongs to the one user thread at its root rather than to its immediate parent, across both spawn schema eras, and each agent reports the number of hops back to that root as its spawn depth.

## Codex Turn Tail Parsing

A turn's own records can push its `task_started` out of the scan window, and a window holding no boundary means the tail is still inside a turn; a record still mid-write is skipped rather than read as half a boundary.

## Codex Idle Cutoff

A rollout that died mid-turn leaves an unmatched `task_started` forever, so silence past the cutoff is the only evidence it is gone; a whole thread tree that goes quiet leaves the scan entirely.

## Observed Model Aggregation

Open agents aggregate equal validated model ids, exclude closed agents from both the count and the model lookup, retain malformed ids as unknown, and keep group totals equal to the exact open count.

## Retained Agent Model Lookup

Claude child-model lookup selects the latest exact derived model for the requested provider, root session, and agent id without leaking evidence from another root.

## Claude Retained Model Resolution

Claude open agents resolve only from exact retained child evidence; delayed ingestion may refresh that evidence, while missing evidence stays unknown and no root model is inferred.

## Fused Merge Snapshot Consistency

A scan that swaps open membership during retained-model resolution cannot mix a lock-time count with model groups from a later registry generation.

## Retained Resolution After Registry Unlock

Retained-model resolution can re-enter the observed registry, proving database work starts only after the merge guard drops.

## Sessions SQL Excludes Historical Agent State

The storage query keeps parent and subagent usage totals but initializes the live count as null and contains no historical agent projection or audit-table reconstruction.

## Audit Persistence Is Non-Authoritative

Sibling hook fires sharing one timestamp retain distinct audit identities, confirming the audit table records fires rather than reconstructing any live count.

## Nullable Sessions IPC Overlay

Command-layer enrichment overlays exact provider, host, and root matches while unmatched storage rows serialize count and model groups as null.

## Observed-Only Session Merge

An active root with validated root cwd synthesizes before token storage, advances a retained row's activity from the scan, and merges into that row without duplication.

## Observed-Only Merge Boundaries

Synthetic rows require a validated root cwd and obey normalized hostname and provider filters, selected range, deterministic limit, provider disable, and global tracking disable.

## Claude Managed Observer Hooks

Claude setup registers the tool-phase observer only under activity tracking and registers no session or subagent lifecycle group at all, so none is ever written to user settings.

## Codex Audit Payloads

Codex payload construction preserves event, tool, root and agent identity, hostname normalization, legacy session fallbacks, and malformed-evidence safety, carrying no field the audit row does not store.

## Codex Managed Observer Hooks

Codex integration verification registers the observer on exactly the seven observed events, never on a lifecycle event, and removes it when activity tracking is disabled.

## Positive-Only Sessions Rows

Frontend formatting and fixtures omit null and zero, render known model families as short labels, preserve unrecognized raw ids, reconcile missing evidence into a final `?` group, and keep positive evidence visible on idle rows.

## Observed-Only Sessions Presentation

Frontend provenance formatting renders unavailable synthetic tokens as an em dash and omits the false zero-turn tooltip claim while retained rows keep real metrics.
