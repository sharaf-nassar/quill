---
lat:
  require-code-mention: true
---

# Pi Extension Test Specs

These tests pin the managed Pi extension's persisted tracking reporter, local-only tools, routing, root/child capability boundary, and real-loader compatibility.

## Self disabling load

Missing, malformed, or non-loopback Quill config registers no tools or lifecycle handlers and causes no Pi load failure.

## No Quill inertness

Invalid config causes no disk writes or registrations and emits one discoverable notice per attempted Pi home.

## Tracking registration

The extension registers every supported Pi lifecycle, agent, turn, message, tool-execution, model-select, and input handler.

## Reporter coexistence

Compatible managed and npm copies elect one reporter, so every handler and stable lifecycle, model, and usage event registers or emits once.

## Tracking envelopes

Persistent-session handlers push protocol-v2 lifecycle and content-free protocol-v1 analytics acceleration.

Assistant runtime closes on the first text-bearing assistant message rather than turn completion, so tool-only assistant messages do not invent `asst_text`. Quill derives stable live identity from provider and session identity.

## Protocol v2 fixture contract

Deterministic TypeScript builders freeze the exact protocol-v2 generation and persisted-entry wire.

The checked-in JSONL covers canonical identity, lifecycle occurrence fields, every start/end reason, delivery source, lineage state, optional field, exact-generation mismatch, and typed outcome. Valid `quill-tracking` entries contain no prompt, message, or tool output. Lifecycle live emission and persisted entries use protocol 2; native-derived model, usage, activity, and runtime acceleration remains protocol 1.

## Persisted lifecycle evidence

A persistent session appends `quill-tracking` lifecycle/direct-lineage data through Pi before sending the same event UUID in its protocol-v2 live envelope. Pi buffers pre-assistant entries and flushes them with the native session file.

## No-session tracking boundary

A session without a persistent Pi file appends and sends no tracking or health evidence while retaining the root process's eight tools and context router.

## Typed bounded delivery

Lifecycle delivery has bounded retry, reload, periodic recovery, reannouncement, and inert-mismatch behavior.

A persistent live session replays its exact start envelope every 30 seconds without appending another durable entry. Replay ticks skip an overlapping send and stop at shutdown, reporter release, or protocol mismatch; no-session and disabled reporters schedule nothing. Timeout, `429`, and `503` retry once; `401` reloads config once; `409 unknown_session` reannounces the persisted start once before one mutation replay; `426` makes later live pushes inert while durable entries continue. Hook and routing telemetry inspect non-2xx bodies and surface typed server codes without changing routing decisions or escaping handlers.

## Handshake and lineage

Session start appends and sends protocol-v2 lifecycle evidence, resolves one stable parent header id, and notifies indexing only when a transcript path exists.

## Deferred transcript notify

Pi names the session file at start but writes it only once the first assistant message lands, so a named-but-absent transcript sends the lifecycle handshake without notifying; turn end delivers the deferred notify once the file exists.

## Environment Agent Lineage

A Pi process marked as a subagent uses its validated parent-session environment id when its fresh session header has no parent path, and pushes explicit agent lineage to tracking and search notify.

## Invalid Environment Agent Lineage

A marked Pi subagent with an unusable parent-session environment id pushes unresolved proof instead of masquerading as a root session.

## Invalid Header Agent Lineage

A marked Pi subagent rejects malformed or self-referential parent ids read from a session header and pushes unresolved proof instead.

## Turn-end search freshness

After Pi persists a turn, the extension repeats notify with the transcript identity and lineage captured at session start.

## Process lifecycle identity

Reloading the extension in one Pi process preserves its process-instance UUID and advances the per-instance lifecycle sequence; distinct occurrences keep distinct event UUIDs.

## Persisted source durability

Failed live tracking leaves lifecycle evidence in Pi's buffered/persisted session source and creates no failure spool or extension log.

## Tracking capability boundary

Root persistent and no-session modes expose exactly eight `quill_` tools plus context routing. `PI_SUBAGENT_CHILD=1` registers tracking only, with no child tools or router.

Generic launcher configuration may explicitly load the Quill extension and provide best-effort runtime acknowledgement. Quill does not auto-inject the broker-selected path or pin a Quill-specific launcher release; ambient-disabled children without explicit Quill configuration remain unsupported.

## Tool registration boundary

Pi receives the eight `quill_`-prefixed tools with dependency-free plain JSON Schema parameter objects.

## Feature gates

Rendered `context_preservation`, `activity_tracking`, and `context_telemetry` values independently gate tools, routing, and telemetry while retaining only the I/O-free singleton teardown handler.

## Exception containment

Typed registration, persistence, transport, protocol, and config failures remain contained so Pi keeps running while Quill is unavailable or incompatible.

## HTTP tool contract

History and working-context tools preserve loopback hostname semantics, authenticate requests, map parameters, and return typed results.

## Bounded History Results

Pi requests compact history responses, removes duplicate detail payloads, and applies the shared byte ceiling even when an older backend returns oversized hits.

## Bounded synchronous work

Tool and telemetry handlers start their bounded HTTP work and return from synchronous work within 10 milliseconds.

## Telemetry mapping and timeout

Pi telemetry has one canonical tool pair and settled root/child stop semantics.

`tool_execution_start` maps to `PreToolUse` and `tool_execution_end` maps to `PostToolUse`; `tool_call` and `tool_result` never duplicate it. Root `agent_settled` emits `Stop`, while configured child `agent_start`/`agent_settled` exclusively emit `SubagentStart`/`SubagentStop`; turn completion emits neither.

Every authenticated telemetry request carries the elected reporter's normalized host, process instance, install channel, and exact generation headers and uses Codex's exact local timeout value.

## Context router parity

Pi applies the canonical context-router cases that fit Pi tool names, including `fetch_content`, raw fetch denial, API/page guidance, fetch-to-file taint, reader boundaries, session isolation, and the 256-path cap.

## Ready URL rewrites

Every blocked URL has a nonempty reason with an exact `quill_fetch_and_index(url=...)` call; `fetch_content` preserves its single `url` or every entry in `urls`.

## Routing feature gate

Rendering context preservation off registers no tools or routing handler and emits no routing telemetry, even when context telemetry stays on.

## Routing telemetry

Routing denials post provider `pi`, category `routing`, and zero values for every routing token estimate; disabling context telemetry leaves routing active without posts.

## Routing telemetry containment

A synchronous or asynchronous context-savings request failure cannot escape the handler or change its deny result.

## Sustained event load

The reporter sustains 1,000 events per minute for a configurable ten-minute run with sub-10 ms handler work and bounded RSS growth.

## Privacy-Safe Tracking Baseline

The isolated baseline harness measures the current extension handler and 64-child synthetic loopback, persisted-source, SQLite/WAL, reconciliation, and Sessions fixtures.

It reports aggregate statistics only, cleans every temporary artifact, and never starts or touches Quill's live window or runtime state.

## Real Pi session

Installed Pi 0.84.2 loads the extension, flushes a `quill-tracking` custom entry with the native JSONL, pushes matching tracking/runtime envelopes, and calls `quill_context_stats` in an isolated persisted session.
