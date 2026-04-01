#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const https = require("https");
const http = require("http");

function truncate(value, maxLen = 2048) {
  if (value === undefined || value === null) return null;
  const str = typeof value === "object" ? JSON.stringify(value) : String(value);
  return str.length > maxLen ? str.slice(0, maxLen) : str;
}

function loadConfig() {
  const configPath = path.join(
    process.env.HOME || process.env.USERPROFILE,
    ".config",
    "quill",
    "config.json",
  );
  return JSON.parse(fs.readFileSync(configPath, "utf8"));
}

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);

    const phaseMap = { PreToolUse: "pre", PostToolUse: "post" };
    const hookPhase = phaseMap[input.hook_event_name];
    if (!hookPhase || input.tool_name !== "Bash") return;

    const config = loadConfig();
    const payload = JSON.stringify({
      provider: "codex",
      session_id: input.session_id,
      hook_phase: hookPhase,
      tool_name: input.tool_name,
      tool_input: truncate(input.tool_input?.command ?? input.tool_input),
      tool_output: truncate(input.tool_response),
      cwd: input.cwd,
    });

    const url = new URL(`${config.url}/api/v1/learning/observations`);
    const mod = url.protocol === "https:" ? https : http;

    const req = mod.request(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${config.secret}`,
        "Content-Length": Buffer.byteLength(payload),
      },
    }, (res) => {
      if (res.statusCode >= 400 && process.env.QUILL_DEBUG) {
        console.error(`codex observe: server returned ${res.statusCode}`);
      }
      res.resume();
    });

    req.on("error", (err) => {
      if (process.env.QUILL_DEBUG) {
        console.error("codex observe: request error:", err.message);
      }
    });
    req.end(payload);
  } catch (err) {
    if (process.env.QUILL_DEBUG) {
      console.error("codex observe: error:", err.message);
    }
  }
}

main();
