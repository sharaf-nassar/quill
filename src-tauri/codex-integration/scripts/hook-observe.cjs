#!/usr/bin/env node
"use strict";

// quill-managed-observer-payload: 4
// Feature 009 — Codex hook event observer.
//
// Codex rollout JSONL transcripts do not record hook executions, so the
// Quill installer registers this single-purpose script on the observed
// Codex hook events (PreToolUse, PermissionRequest, PostToolUse,
// PreCompact, PostCompact, UserPromptSubmit, Stop). On each invocation the
// script POSTs one event record to /api/v1/hooks/observed, then
// exits with code 0 so it never blocks the hook chain. The endpoint
// fast-acks 202 ACCEPTED, persists in the background, and emits a
// `hooks-observed-updated` Tauri event so the Now-tab Hooks breakdown
// refreshes within a couple of seconds.
//
// Session and subagent lifecycle events are deliberately not registered:
// live session and agent state comes from rollout transcripts, so this
// path is audit history only.
//
// Deployment is gated on the IntegrationFeatures.activity_tracking
// flag in src-tauri/src/integrations/codex.rs.

const fs = require("fs");
const os = require("os");
const { loadConfig, postJSON } = require("./lib.cjs");

function buildPayload(
  input,
  config = {},
  now = new Date(),
  systemHostname = os.hostname(),
) {
  const event = input.hook_event_name;
  if (!event) return null;
  const sessionId = input.session_id || input.conversation_id || input.id || "";
  const configuredHostname =
    typeof config.hostname === "string" ? config.hostname.trim() : "";

  return {
    provider: "codex",
    session_id: sessionId,
    hook_event: event,
    tool_name: input.tool_name || null,
    cwd: input.cwd || null,
    hostname:
      configuredHostname ||
      String(systemHostname || "").split(".")[0] ||
      "local",
    ts: now.toISOString(),
    hook_matcher: null,
    agent_id: input.agent_id || null,
  };
}

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);
    if (!input.hook_event_name) return;
    const config = loadConfig();
    const payload = buildPayload(input, config);
    postJSON(config, "/api/v1/hooks/observed", payload, "codex hook-observe");
  } catch (err) {
    if (process.env.QUILL_DEBUG) {
      console.error("codex hook-observe: error:", err.message);
    }
  }
}

if (require.main === module) main();

module.exports = { buildPayload };
