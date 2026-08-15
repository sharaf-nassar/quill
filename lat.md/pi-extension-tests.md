---
lat:
  require-code-mention: true
---
# Pi Extension Test Specs

These tests pin the managed Pi extension's tracking reporter, private failure spool, local-only tools, routing, and real-loader compatibility.

## Self disabling load

Missing, malformed, or non-loopback Quill config registers no tools or lifecycle handlers and causes no Pi load failure.

## No Quill inertness

Invalid config causes no disk writes or registrations and emits one discoverable notice per attempted Pi home.

## Tracking registration

The extension registers every supported Pi lifecycle, agent, turn, message, tool-execution, model-select, and input handler.

## Tracking envelopes

Handlers push versioned lifecycle, per-message usage, model, activity, and source-less runtime envelopes without message bodies.

## Handshake and lineage

Session start sends the handshake, resolves one stable parent header id, and notifies indexing only when a transcript path exists.

## Stable teardown identity

Reloading the extension for the same session header reproduces the same lifecycle event id so retries remain idempotent across teardown.

## Spool durability

Failed tracking sends lazily create a bounded 0700 spool with capped 0600 per-session files and a bounded private diagnostic log.

## Protocol degradation

Protocol mismatch becomes a typed, logged, spooled failure without escaping into Pi or starting an unbounded retry loop.

## Tool registration boundary

Pi receives the eight `quill_`-prefixed tools with dependency-free plain JSON Schema parameter objects.

## Feature gates

Rendered `context_preservation`, `activity_tracking`, and `context_telemetry` values independently gate tools, routing, and lifecycle telemetry.

## Exception containment

Typed registration, transport, protocol, config, and spool failures remain contained so Pi keeps running while Quill is unavailable or incompatible.

## HTTP tool contract

History and working-context tools preserve loopback hostname semantics, authenticate requests, map parameters, and return typed results.

## Bounded synchronous work

Tool and telemetry handlers start their bounded HTTP work and return from synchronous work within 10 milliseconds.

## Telemetry mapping and timeout

Pi lifecycle events map to the existing hook vocabulary with provider `pi`, and every request uses Codex's exact local timeout value.

## Context router parity

Pi applies the canonical context-router cases that fit Pi tool names, including raw fetch denial, API/page guidance, fetch-to-file taint, reader boundaries, session isolation, and the 256-path cap.

## Ready URL rewrites

Every blocked URL has a nonempty reason with an exact `quill_fetch_and_index(url=...)` call that preserves the extracted URL.

## Routing feature gate

Rendering context preservation off registers no tools or routing handler and emits no routing telemetry, even when context telemetry stays on.

## Routing telemetry

Routing denials post provider `pi`, category `routing`, and zero values for every routing token estimate; disabling context telemetry leaves routing active without posts.

## Routing telemetry containment

A synchronous or asynchronous context-savings request failure cannot escape the handler or change its deny result.

## Sustained event load

The reporter sustains 1,000 events per minute for a configurable ten-minute run with sub-10 ms handler work and bounded RSS growth.

## Real Pi session

Installed Pi 0.84.2 loads the extension, pushes tracking and runtime envelopes, and calls `quill_context_stats` in an isolated persisted session.
