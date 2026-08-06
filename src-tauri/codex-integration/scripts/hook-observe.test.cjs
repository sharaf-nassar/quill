#!/usr/bin/env node
"use strict";

const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { mkdtempSync, rmSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const test = require("node:test");

const { buildPayload } = require("./hook-observe.cjs");

const now = new Date("2026-08-04T12:34:56.789Z");

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Lifecycle Payloads]]
test("builds complete lifecycle payloads from official Codex fields", () => {
  for (const source of ["startup", "resume", "clear", "compact"]) {
    const payload = buildPayload(
      {
        hook_event_name: "SessionStart",
        session_id: "root-session",
        source,
        cwd: "/workspace",
      },
      { hostname: "configured-host" },
      now,
    );

    assert.deepEqual(payload, {
      provider: "codex",
      session_id: "root-session",
      hook_event: "SessionStart",
      tool_name: null,
      cwd: "/workspace",
      hostname: "configured-host",
      source,
      ts: "2026-08-04T12:34:56.789Z",
      hook_matcher: null,
      agent_id: null,
      model: null,
    });
  }

  const subagent = buildPayload(
      {
        hook_event_name: "SubagentStart",
        session_id: "root-session",
        agent_id: "agent-1",
        model: "gpt-5.6-sol",
      },
      { hostname: "configured-host" },
      now,
    );
  assert.equal(subagent.agent_id, "agent-1");
  assert.equal(subagent.model, "gpt-5.6-sol");

  assert.equal(buildPayload({
    hook_event_name: "SubagentStart",
    session_id: "root-session",
    agent_id: "agent-2",
    model: { invalid: true },
  }, { hostname: "configured-host" }, now).model, null);
});

// @lat: [[live-subagent-count-tests#Live Subagent Count Tests#Codex Subagent Stop Response]]
test("SubagentStop returns a valid no-op response", () => {
  const payload = buildPayload({
    hook_event_name: "SubagentStop",
    session_id: "root-session",
    agent_id: "agent-1",
  }, { hostname: "configured-host" }, now);
  assert.equal(payload.agent_id, "agent-1");

  const home = mkdtempSync(join(tmpdir(), "quill-hook-observe-"));
  try {
    const result = spawnSync(process.execPath, [join(__dirname, "hook-observe.cjs")], {
      encoding: "utf8",
      env: { ...process.env, HOME: home, USERPROFILE: home },
      input: JSON.stringify({
        hook_event_name: "SubagentStop",
        session_id: "root-session",
        agent_id: "agent-1",
      }),
    });
    assert.equal(result.status, 0);
    assert.deepEqual(JSON.parse(result.stdout), {});
  } finally {
    rmSync(home, { force: true, recursive: true });
  }
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

test("malformed lifecycle evidence cannot claim a covered epoch", () => {
  assert.equal(buildPayload({}, {}, now), null);

  for (const source of [undefined, "startup", "fork", "STARTUP"]) {
    const payload = buildPayload(
      { hook_event_name: "SessionStart", source },
      {},
      now,
      "",
    );
    assert.equal(payload.session_id, "");
    assert.equal(payload.hostname, "local");
    assert.equal(payload.source, null);
  }

  assert.equal(
    buildPayload(
      { hook_event_name: "SubagentStart", source: "startup" },
      {},
      now,
      "host",
    ).source,
    null,
  );
});
