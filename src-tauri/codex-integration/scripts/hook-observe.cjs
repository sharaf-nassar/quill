#!/usr/bin/env node
"use strict";

// quill-managed-observer-payload: 2
// Feature 009 — Codex hook event observer.
//
// Codex rollout JSONL transcripts do not record hook executions, so the
// Quill installer registers this single-purpose script on every Codex
// hook event (PreToolUse, PostToolUse, SessionStart, UserPromptSubmit,
// Stop, PreCompact, PostCompact, PermissionRequest, SubagentStart,
// SubagentStop, SessionEnd). On each invocation the script POSTs one event
// record to /api/v1/hooks/observed, then
// exits with code 0 so it never blocks the hook chain. The endpoint
// fast-acks 202 ACCEPTED, persists in the background, and emits a
// `hooks-observed-updated` Tauri event so the Now-tab Hooks breakdown
// refreshes within a couple of seconds.
//
// Deployment is gated on the IntegrationFeatures.activity_tracking
// flag in src-tauri/src/integrations/codex.rs.

const fs = require("fs");
const os = require("os");
const path = require("path");
const https = require("https");
const http = require("http");

const LOCAL_TIMEOUT_MS = 1500;
const REMOTE_TIMEOUT_MS = 2000;
// https://developers.openai.com/codex/config-advanced#sessionstart
const SESSION_START_SOURCES = new Set([
  "startup",
  "resume",
  "clear",
  "compact",
]);

function loadConfig() {
  const configPath = path.join(
    process.env.HOME || process.env.USERPROFILE,
    ".config",
    "quill",
    "config.json",
  );
  return JSON.parse(fs.readFileSync(configPath, "utf8"));
}

function isLocal(urlStr) {
  return (
    urlStr.includes("localhost") ||
    urlStr.includes("127.0.0.1") ||
    urlStr.includes("[::1]")
  );
}

function postJSON(config, endpoint, payload, label) {
  const body = JSON.stringify(payload);
  const url = new URL(`${config.url}${endpoint}`);
  const mod = url.protocol === "https:" ? https : http;
  const timeoutMs = isLocal(config.url) ? LOCAL_TIMEOUT_MS : REMOTE_TIMEOUT_MS;

  let settled = false;
  let timer;
  const clearTimer = () => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
  };

  const req = mod.request(
    url,
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${config.secret}`,
        "Content-Length": Buffer.byteLength(body),
      },
    },
    (res) => {
      clearTimer();
      if (res.statusCode >= 400 && process.env.QUILL_DEBUG) {
        console.error(`${label}: server returned ${res.statusCode}`);
      }
      res.resume();
    },
  );

  req.on("error", (err) => {
    clearTimer();
    if (process.env.QUILL_DEBUG) {
      console.error(`${label}: request error:`, err.message);
    }
  });
  req.on("close", clearTimer);
  timer = setTimeout(() => {
    req.destroy(new Error(`timed out after ${timeoutMs}ms`));
  }, timeoutMs);
  timer.unref?.();
  req.end(body);
}

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
    source:
      sessionId &&
      event === "SessionStart" &&
      SESSION_START_SOURCES.has(input.source)
        ? input.source
        : null,
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
