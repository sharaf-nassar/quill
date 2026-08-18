---
lat:
  require-code-mention: true
---
# Pi Live Session Test Specs

These tests pin Pi's extension-pushed live sessions after transcript tracking removal.

## Push Lifecycle

Pushed starts create normalized Pi live keys, replacement starts remove the prior identity, and shutdown removes the active identity deterministically.

## Push Continuity

Startup or reload of the same stable id preserves its original start while advancing activity and ignores a shutdown older than the continued session.

## Push Mutations

Activity, model, lineage, and cumulative-token pushes update an existing Pi session but never invent one without lifecycle evidence.

## Push Crash Eviction

A pushed Pi session with no shutdown ages out through the shared 15-minute idle sweep, and a shutdown arriving after eviction stays an idempotent no-op.

## Persisted Source Presentation

Persisted Pi work never renders the retired EPHEMERAL badge. The additive database field remains inert for schema compatibility, while no-file sessions intentionally produce no row.

## Extension Health Persistence

One atomic settings write records the handshake protocol, extension version, minimum Quill version, last report time, and typed last error.

An unchanged handshake repeated inside the refresh window writes nothing, while any changed field writes through at once.

## Tracking Request Validation

The Pi tracking boundary rejects bad bearer authentication with typed `401`, malformed or invalid authenticated bodies with typed `400`, and exact-generation mismatch with typed `426` before lifecycle mutation.

## Authenticated Protocol v2 Router

The real `/api/v1/pi/track` router authenticates before reading a bounded 1 MiB body, feeds exact fixture bytes through the open protocol-v2 decoder, and returns typed `400`, `401`, `409`, `426`, `429`, and `503` responses.

Accepted responses include exact Quill build, protocol, reporter version, capability digest, and ordered dispositions.

## Transactional Lifecycle Disposition

One SQLite transaction returns `applied`, `duplicate`, `stale`, or `unknown_session` for each validated event.

Only committed `applied` events mutate `LiveTracker`; newer process starts supersede older instances, while stale process ends and reconciliation cannot reopen or remove the replacement.

Durable open rows load as recovering after restart or tracking re-enable. Same-process lifecycle or live-hint evidence can prove them live; a mismatched process is stale and an absent or closed lifecycle returns `unknown_session`.

## Live Hint Recovery And Source Diagnostic

Pi session-message hints consult durable lifecycle before analytics mutation, returning typed `409` for unknown or stale ownership.

A live start without a reconciled file records `source_not_persisted`; validated notify or committed persisted-source reconciliation clears only that process's diagnostic.

## Protocol v2 decoder contract

The pure Rust decoder accepts only the exact protocol-v2 generation and persisted-entry schema.

It reads open generation metadata before closed lifecycle/lineage variants, rejects unknown or null optional fields, validates canonical Pi identity and occurrence ordering, and decodes typed accepted, mismatch, and unknown-session responses from the exact TypeScript fixture bytes.

## Agent Lineage Protocol

The Pi tracking protocol accepts explicit agent lineage with a validated parent session id so the extension marker survives the HTTP boundary.

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

## Demo Gate

Demo mode returns a typed unavailable result without changing extension health, durable lifecycle origin, or LiveTracker state.

## Tracking Ingestion

A valid session-start envelope lowercases and shortens its hostname once, then uses that same key for durable lifecycle origin, live state, and extension health.

## Cumulative Token Display

A live Pi row renders the cumulative total maintained by pushed usage instead of replacing it with an em dash.

## Proven Live Lineage

One parent with two live children linked by pushed proof exposes exactly two linked sessions while a pushed root sibling stays independent and Pi native agent count stays unknown.

## Explicit Pi Agent Lineage

An explicitly marked Pi subagent becomes a live agent on its resolved root with model, validated launcher role, count, runtime, and activity while its child session row is omitted; generic Pi links remain separate.

## Depth-Bounded Agent Projection

Nested explicit agents preserve direct lineage internally but flatten into one visible root rail.

Missing parents remain independent unresolved live rows, late proof attaches the same identity, and completion removes the child projection. Family activity and runtime include descendants; agent count/runtime remain explicit; root tokens and turns stay root-only.

## Depth 64 Cycle And Cross-Host Rejection

The memoized lineage resolver supports 64 direct edges. A 65th edge, cycle, missing ancestor, or parent found only on another host stays an unresolved independent live row rather than disappearing or attaching across identity boundaries.

## Pi Agent Retained Parent Overlay

An active explicit Pi agent keeps its otherwise-idle parent live and rankable, so limited storage reads return the parent with the child attached and no independent child.

## Pi Agent Runtime Projection

Native runtime enrichment preserves pushed Pi agent totals and per-agent baselines so storage coverage cannot erase the live fold before IPC serialization.

## Pushed Lineage Proof

Pushed root, linked, and unresolved states remain distinct in the live overlay, including the unresolved reason and the linked parent's stable id.
