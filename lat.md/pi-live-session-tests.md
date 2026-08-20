---
lat:
  require-code-mention: true
---
# Pi Live Session Test Specs

These tests pin Pi protocol-2 lifecycle state alongside persisted session reconciliation.

## Push Lifecycle

Pushed starts create normalized Pi live keys, replacement starts remove the prior identity, and shutdown removes the active identity deterministically.

## Push Continuity

Startup or reload of the same stable id preserves its original start while advancing activity and ignores a shutdown older than the continued session.

## Push Crash Eviction

A pushed Pi session with no shutdown ages out through the shared 15-minute idle sweep, and a shutdown arriving after eviction stays an idempotent no-op.

## Persisted Source Presentation

Persisted Pi work never renders the retired EPHEMERAL badge. The additive database field remains inert for schema compatibility, while no-file sessions intentionally produce no row.

## Authenticated Protocol v2 Router

The real `/api/v1/pi/track` router authenticates and validates protocol-2 lifecycle before mutation.

It bounds the body at 1 MiB, feeds exact fixture bytes through the open protocol-v2 decoder, and returns typed `400`, `401`, `409`, `429`, and `503` responses. Reporter/build/capability metadata remains bounded and non-empty but is not an exact desktop-generation latch.

Accepted responses include current Quill build, protocol, reporter version, capability digest, and ordered dispositions. A legacy protocol-2 generation is accepted; a different protocol receives `400` without reporter-health persistence or reload remediation.

## Extension Track Wire Contract

Every payload builder in the extension that posts to `/api/v1/pi/track` has a generated wire fixture, and the real router answers each one's exact request bytes and lifecycle identity headers.

The lifecycle builders are enumerated from the extension source rather than from the endpoint string, so a new tracking shape fails the suite until it carries a fixture.

Only protocol-2 lifecycle shapes are accepted; reporter, build, and capability generation metadata may be older.

## Transactional Lifecycle Disposition

One SQLite transaction returns `applied`, `duplicate`, `stale`, or `unknown_session` for each validated event.

Only committed `applied` events mutate `LiveTracker`; newer process starts supersede older instances, while stale process ends and reconciliation cannot reopen or remove the replacement.

Durable open rows load as recovering after restart or tracking re-enable. Recently closed rows load only inside the shared idle window, preserving their host, session id, and close instant for tombstone seeding. Same-process lifecycle evidence can prove an open row live; a mismatched process is stale and an absent or closed lifecycle returns `unknown_session`.

## Lifecycle Recovery

A persistent Pi session sends its start once. Fold sweeps recover local persisted sessions, while `409 unknown_session` triggers one targeted start reannouncement before retrying the current lifecycle event.

Durable lifecycle and receipt rows remain for remote-host ordering, idempotency, and lineage; no reporter-health diagnostic row is created.

## Persisted Turn Recovery

Persisted Pi user and assistant messages produce source-owned `response_times`, so removing runtime-message acceleration does not remove turn pairing.
## Protocol v2 decoder contract

The pure Rust decoder accepts only the exact protocol-v2 generation and persisted-entry schema.

It reads open generation metadata before closed lifecycle/lineage variants, rejects unknown or null optional fields, validates canonical Pi identity and occurrence ordering, and decodes typed accepted, mismatch, and unknown-session responses from the exact TypeScript fixture bytes.

## Tracking Rate Headroom

The independent Pi tracking limiter charges contained events and accepts 4,000 events in one 60-second window, four times the specified stream even when envelopes batch 200 events.

## Pi Session Message Rate Isolation

Pi runtime traffic charges contained messages, accepts 4,000 messages per minute, and consumes no session-notify or other-provider capacity.

## Pi Runtime Message Mapping

Pi turn, input, and tool execution types map to canonical runtime events while the unavailable thinking event remains an explicit rejected gap.

## Split Turn Response Pairing

A reply pairs with a prompt pushed in an earlier request, because the extension sends one message per request.

Later replies inside the same turn stay unpaired, so one prompt counts as one turn, and re-pushing a message invents no extra turns.

## Pi Runtime Hostname

Pi runtime messages normalize their hostname to the same lowercase short key used by lifecycle tracking before analytics storage.

## Cumulative Token Display

A live Pi row renders the cumulative total maintained by pushed usage instead of replacing it with an em dash.

## Explicit Pi Agent Lineage

An explicitly marked Pi subagent becomes a live agent on its resolved root with model, validated launcher role, count, runtime, and activity while its child session row is omitted; generic Pi links remain separate.

While any child stays open, the root's activity reads as the overlay instant itself rather than the newest transcript flush, so a root turn-settle terminal older than that instant can never present the rail's session as ended while its agents are still working.

## Depth-Bounded Agent Projection

Nested explicit agents preserve direct lineage internally but flatten into one visible root rail.

Missing parents remain independent unresolved live rows, late proof attaches the same identity, and completion removes the child projection. Family activity and runtime include descendants; agent count/runtime remain explicit; root tokens and turns stay root-only.

## Reporter End Tombstone

A reporter-announced end closes that Pi child occurrence: later sweeps over its still-recent transcript cannot resurrect it through either the warm tail or the cold header path.

A restarted tracker reaches the same answer from durable closed lifecycle rows — seeding drops a child its startup sweep already re-folded while that child claims no process — and only a new `session_start` for the same identity reopens folding, carrying its role with it.

## Depth 64 Cycle And Cross-Host Rejection

The memoized lineage resolver supports 64 direct edges. A 65th edge, cycle, missing ancestor, or parent found only on another host stays an unresolved independent live row rather than disappearing or attaching across identity boundaries.

## Pi Agent Retained Parent Overlay

An active explicit Pi agent keeps its otherwise-idle parent live and rankable, so limited storage reads return the parent with the child attached and no independent child.

## Pi Agent Runtime Projection

Native runtime enrichment preserves pushed Pi agent totals and per-agent baselines so storage coverage cannot erase the live fold before IPC serialization.

## Pushed Lineage Proof

Pushed root, linked, and unresolved states remain distinct in the live overlay, including the unresolved reason and the linked parent's stable id.
