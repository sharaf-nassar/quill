#!/usr/bin/env node
"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const { buildPayload } = require("./hook-observe.cjs");

const now = new Date("2026-08-04T12:34:56.789Z");

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Audit Payloads]]
test("builds complete audit payloads from official Codex fields", () => {
  const payload = buildPayload(
    {
      hook_event_name: "PreToolUse",
      session_id: "root-session",
      tool_name: "Bash",
      cwd: "/workspace",
      agent_id: "agent-1",
    },
    { hostname: "configured-host" },
    now,
  );

  assert.deepEqual(payload, {
    provider: "codex",
    session_id: "root-session",
    hook_event: "PreToolUse",
    tool_name: "Bash",
    cwd: "/workspace",
    hostname: "configured-host",
    ts: "2026-08-04T12:34:56.789Z",
    hook_matcher: null,
    agent_id: "agent-1",
  });
});

test("preserves legacy session fallbacks and normalizes fallback hostname", () => {
  const conversation = buildPayload(
    {
      hook_event_name: "PreToolUse",
      conversation_id: "legacy-conversation",
      tool_name: "Bash",
      cwd: "/legacy",
    },
    {},
    now,
    "worker.example.com",
  );

  assert.equal(conversation.session_id, "legacy-conversation");
  assert.equal(conversation.hostname, "worker");
  assert.equal(conversation.tool_name, "Bash");
  assert.equal(conversation.cwd, "/legacy");
  assert.equal(conversation.hook_matcher, null);
  assert.equal(
    buildPayload({ hook_event_name: "Stop", id: "legacy-id" }, {}, now, "")
      .session_id,
    "legacy-id",
  );
});

test("malformed evidence stays identifiable without inventing fields", () => {
  assert.equal(buildPayload({}, {}, now), null);

  const payload = buildPayload({ hook_event_name: "Stop" }, {}, now, "");
  assert.equal(payload.session_id, "");
  assert.equal(payload.hostname, "local");
  assert.equal(payload.agent_id, null);
});
