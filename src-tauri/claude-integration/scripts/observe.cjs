#!/usr/bin/env node
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const https = require("https");
const http = require("http");
const LOCAL_TIMEOUT_MS = 1500;
const REMOTE_TIMEOUT_MS = 2000;

function truncate(value, maxLen = 2048) {
  if (value === undefined || value === null) return null;
  const str = typeof value === "object" ? JSON.stringify(value) : String(value);
  return str.length > maxLen ? str.slice(0, maxLen) : str;
}

function isLocal(urlStr) {
  return urlStr.includes("localhost") || urlStr.includes("127.0.0.1") || urlStr.includes("[::1]");
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

  const req = mod.request(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${config.secret}`,
      "Content-Length": Buffer.byteLength(body),
    },
  }, (res) => {
    clearTimer();
    if (res.statusCode >= 400 && process.env.QUILL_DEBUG) {
      console.error(`${label}: server returned ${res.statusCode}`);
    }
    res.resume();
  });

  req.on("error", (err) => {
    clearTimer();
    if (process.env.QUILL_DEBUG) console.error(`${label}: request error:`, err.message);
  });
  req.on("close", clearTimer);
  timer = setTimeout(() => {
    req.destroy(new Error(`timed out after ${timeoutMs}ms`));
  }, timeoutMs);
  timer.unref?.();
  req.end(body);
}

function buildPayload(input) {
  const phaseMap = {
    PreToolUse: "pre",
    PostToolUse: "post",
    PostToolUseFailure: "post",
  };
  const hookPhase = phaseMap[input.hook_event_name];
  if (!hookPhase) return null;

  // Skip low-signal PreToolUse hooks (post-phase captures errors/results)
  const LOW_SIGNAL_PRE = ["Read", "Glob", "Grep", "Bash", "LS", "WebSearch", "WebFetch", "Agent"];
  if (hookPhase === "pre" && LOW_SIGNAL_PRE.includes(input.tool_name)) return null;

  // Post-phase only records tools whose outcome teaches the learner something —
  // edits, writes, and shell commands. Reads, lookups, MCP calls, and meta tools
  // generate ~50% of observations with no behavioral signal; the audit showed
  // 26k observations/7d but only ~10 useful tool kinds.
  const HIGH_SIGNAL_POST = new Set([
    "Bash",
    "Edit",
    "Write",
    "NotebookEdit",
  ]);
  if (hookPhase === "post" && !HIGH_SIGNAL_POST.has(input.tool_name)) return null;

  const toolOutput = input.hook_event_name === "PostToolUseFailure"
    ? {
        error: input.error,
        is_interrupt: input.is_interrupt ?? false,
        duration_ms: input.duration_ms ?? null,
      }
    : input.tool_response;
  return {
    provider: "claude",
    session_id: input.session_id,
    hook_phase: hookPhase,
    tool_name: input.tool_name,
    tool_input: truncate(input.tool_input),
    tool_output: truncate(toolOutput),
    cwd: input.cwd,
  };
}

const TERMINAL_EVENTS = new Set(["Stop", "StopFailure", "SessionEnd"]);

function buildTerminalPayload(
  input,
  config = {},
  ts = new Date().toISOString(),
  systemHostname = os.hostname(),
) {
  const hookEvent = input?.hook_event_name;
  if (!TERMINAL_EVENTS.has(hookEvent) || !input.session_id) return null;
  const configuredHostname =
    typeof config.hostname === "string" ? config.hostname.trim() : "";
  return {
    provider: "claude",
    session_id: input.session_id,
    hostname:
      configuredHostname ||
      String(systemHostname || "").split(".")[0] ||
      "local",
    hook_event: hookEvent,
    tool_name: null,
    cwd: input.cwd || null,
    ts,
    hook_matcher: null,
    agent_id: null,
  };
}

function loadConfig() {
  const configPath = path.join(
    process.env.HOME || process.env.USERPROFILE,
    ".config",
    "quill",
    "config.json"
  );
  return JSON.parse(fs.readFileSync(configPath, "utf8"));
}

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);

    if (TERMINAL_EVENTS.has(input?.hook_event_name)) {
      const config = loadConfig();
      const payload = buildTerminalPayload(input, config);
      if (payload !== null) {
        postJSON(config, "/api/v1/hooks/observed", payload, "observe");
      }
      return;
    }

    const payload = buildPayload(input);
    if (payload === null) return;

    const config = loadConfig();
    postJSON(config, "/api/v1/learning/observations", payload, "observe");
  } catch (err) {
    if (process.env.QUILL_DEBUG) console.error("observe: error:", err.message);
  }
}

if (require.main === module) main();

module.exports = { buildPayload, buildTerminalPayload, truncate };
