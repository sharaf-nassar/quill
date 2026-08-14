---
lat:
  require-code-mention: true
---
# Pi Extension Test Specs

These tests pin the managed Pi extension's local-only tools, feature gates, failure containment, telemetry, and real-loader compatibility.

## Self disabling load

Missing, malformed, or non-loopback Quill config registers no tools or lifecycle handlers and causes no Pi load failure.

## Tool registration boundary

Pi receives the eight `quill_`-prefixed tools with dependency-free plain JSON Schema parameter objects.

## Feature gates

Rendered `context_preservation`, `activity_tracking`, and `context_telemetry` values independently gate tools, routing, and lifecycle telemetry.

## Exception containment

Every registration, tool handler, and telemetry handler contains thrown failures so Pi keeps running while Quill is unavailable or incompatible.

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

## Real Pi session

Installed Pi 0.84.1 loads the extension and calls `quill_context_stats` through an isolated persisted session against loopback probe servers.
