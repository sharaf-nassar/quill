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

## Ephemeral Persistence

Migration 42 and the lifecycle upsert preserve the ephemeral flag with Pi's cwd and normalized hostname. Migration 44 orders replayed close/start events so stale starts stay closed and newer resumes reopen.

## Ephemeral Live Overlay

A pushed ephemeral start marks its live Sessions row immediately, before any usage or transcript evidence exists.

## Ephemeral Breakdown Persistence

An ephemeral lifecycle origin remains a Sessions row after shutdown and combines pushed usage with session-owned turn activity without creating ordinary lifecycle-only rows.

## Ephemeral Badge

The Sessions identity renders a neutral, accessible EPHEMERAL badge only for rows whose additive breakdown flag is true.

## Extension Health Persistence

One atomic settings write records the handshake protocol, extension version, minimum Quill version, last report time, and typed last error.

An unchanged handshake repeated inside the refresh window writes nothing, while any changed field writes through at once.

## Tracking Request Validation

The Pi tracking boundary rejects bad bearer authentication with `401`, protocol mismatch with a typed `400`, and control characters in hostname identity before mutation.

## Agent Lineage Protocol

The Pi tracking protocol accepts explicit agent lineage with a validated parent session id so the extension marker survives the HTTP boundary.

## Tracking Rate Headroom

The independent Pi tracking limiter charges contained events and accepts 4,000 events in one 60-second window, four times the specified stream even when envelopes batch 200 events.

## Pi Session Message Rate Isolation

Pi runtime traffic charges contained messages, accepts 4,000 messages per minute, and consumes no session-notify or other-provider capacity.

## Pi Runtime Message Mapping

Pi turn, input, and tool execution types map to canonical runtime events while the unavailable thinking event remains an explicit rejected gap.

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

An explicitly marked Pi subagent becomes a live agent on its parent with model, count, runtime, and activity while its child session row is omitted; generic Pi links remain separate.

## Pi Agent Retained Parent Overlay

An active explicit Pi agent keeps its otherwise-idle parent live and rankable, so limited storage reads return the parent with the child attached and no independent child.

## Pi Agent Runtime Projection

Native runtime enrichment preserves pushed Pi agent totals and per-agent baselines so storage coverage cannot erase the live fold before IPC serialization.

## Pushed Lineage Proof

Pushed root, linked, and unresolved states remain distinct in the live overlay, including the unresolved reason and the linked parent's stable id.
