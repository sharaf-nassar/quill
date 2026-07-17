#!/usr/bin/env node
"use strict";

// Feature 009 — Codex hook event observer.
//
// Codex rollout JSONL transcripts do not record hook executions, so the
// Quill installer registers this single-purpose script on every Codex
// hook event (PreToolUse, PostToolUse, SessionStart, UserPromptSubmit,
// Stop, PreCompact, PostCompact, PermissionRequest, SubagentStart,
// SubagentStop). On each invocation
// the script POSTs one event record to /api/v1/hooks/observed, then
// exits with code 0 so it never blocks the hook chain. The endpoint
// fast-acks 202 ACCEPTED, persists in the background, and emits a
// `hooks-observed-updated` Tauri event so the Now-tab Hooks breakdown
// refreshes within a couple of seconds.
//
// Deployment is gated on the IntegrationFeatures.activity_tracking
// flag in src-tauri/src/integrations/codex.rs.

const fs = require("fs");
const path = require("path");
const https = require("https");
const http = require("http");

const LOCAL_TIMEOUT_MS = 1500;
const REMOTE_TIMEOUT_MS = 2000;

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

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);

    const event = input.hook_event_name;
    if (!event) return;
    const config = loadConfig();
    const payload = {
      provider: "codex",
      session_id: input.session_id || input.conversation_id || input.id || "",
      hook_event: event,
      tool_name: input.tool_name || null,
      cwd: input.cwd || null,
      ts: new Date().toISOString(),
      hook_matcher: input.matcher || null,
      agent_id: input.agent_id || null,
    };
    postJSON(config, "/api/v1/hooks/observed", payload, "codex hook-observe");
  } catch (err) {
    if (process.env.QUILL_DEBUG) {
      console.error("codex hook-observe: error:", err.message);
    }
  }
}

main();
