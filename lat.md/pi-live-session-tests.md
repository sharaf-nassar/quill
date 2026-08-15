---
lat:
  require-code-mention: true
---
# Pi Live Session Test Specs

These tests pin Pi's pushed live-session foundation while the bounded transcript fold remains available until the final cut-over.

## Push Lifecycle

Pushed starts create normalized Pi live keys, replacement starts remove the prior identity, and shutdown removes the active identity deterministically.

## Push Continuity

Startup or reload of the same stable id preserves its original start while advancing activity and ignores a shutdown older than the continued session.

## Push Mutations

Activity, model, lineage, and cumulative-token pushes update an existing Pi session but never invent one without lifecycle evidence.

## Push Crash Eviction

A pushed Pi session with no shutdown ages out through the shared 15-minute idle sweep, and a shutdown arriving after eviction stays an idempotent no-op.

## Ephemeral Persistence

Migration 42 and the lifecycle upsert preserve the ephemeral flag with Pi's cwd and normalized hostname in `live_analytics_sessions`.

## Extension Health Persistence

One atomic settings write records the handshake protocol, extension version, minimum Quill version, last report time, and typed last error.

## Tracking Request Validation

The Pi tracking boundary rejects bad bearer authentication with `401` and protocol mismatch with a typed `400` response.

## Tracking Rate Headroom

The independent Pi tracking limiter accepts 4,000 single-event envelopes in one 60-second window, four times the specified 1,000-event stream.

## Pi Session Message Rate Isolation

Pi runtime message traffic accepts the 1,000-event/minute load without consuming capacity from session notify or other providers' message traffic.

## Pi Runtime Message Mapping

Pi turn, input, and tool execution types map to canonical runtime events while the unavailable thinking event remains an explicit rejected gap.

## Demo Gate

Demo mode returns a typed unavailable result without changing extension health, durable lifecycle origin, or LiveTracker state.

## Tracking Ingestion

A valid session-start envelope lowercases and shortens its hostname once, then uses that same key for durable lifecycle origin, live state, and extension health.

## Liveness

A valid Pi session file creates one provider-isolated live session from transcript evidence.

## Header Cwd

The live session takes cwd from the Pi session header rather than decoding its lossy directory name.

## Last Entry Timestamp

The newest complete tail entry supplies Pi activity time without consulting file mtime.

## Last Message Identity

The newest assistant message supplies its validated upstream provider and model without walking parent links.

## Deferred Initial Flush

An absent path remains a no-op until Pi's deferred first flush creates the transcript, which then folds normally.

## Equal Length Rewrite

An `(mtime_ns, len)` change cold-refolds Pi state even when a migration rewrite preserves file length.

## Idle Quiescence

A Pi transcript silent beyond the shared 15-minute cutoff releases its live session and file state.

## Cumulative Token Display

A live Pi row renders the cumulative total maintained by pushed usage instead of replacing it with an em dash.

## Ephemeral No Op

A Pi conversation with no session file produces no live row.

## Bounded Large Branch Tail

A synthetic branched Pi session of 104,857,951 bytes scans 1,048,576 bytes, the shared tail bound, from an isolated temporary file.

## Proven Live Lineage

One parent with two live declared children exposes exactly two linked sessions while an unlinked sibling stays independent and Pi native agent count stays unknown.
