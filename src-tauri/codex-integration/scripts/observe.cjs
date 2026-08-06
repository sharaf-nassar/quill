#!/usr/bin/env node
"use strict";

const fs = require("fs");
const { loadConfig, postJSON } = require("./lib.cjs");

function truncate(value, maxLen = 2048) {
  if (value === undefined || value === null) return null;
  const str = typeof value === "object" ? JSON.stringify(value) : String(value);
  return str.length > maxLen ? str.slice(0, maxLen) : str;
}

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);

    const phaseMap = { PreToolUse: "pre", PostToolUse: "post" };
    const hookPhase = phaseMap[input.hook_event_name];
    // Only observe shell-style tool calls; mirrors the Bash|apply_patch
    // matcher the installer registers this observer on.
    const observedTools = new Set(["Bash", "apply_patch"]);
    if (!hookPhase || !observedTools.has(input.tool_name)) return;

    const config = loadConfig();
    const payload = {
      provider: "codex",
      session_id: input.session_id,
      hook_phase: hookPhase,
      tool_name: input.tool_name,
      tool_input: truncate(input.tool_input?.command ?? input.tool_input),
      tool_output: truncate(input.tool_response),
      cwd: input.cwd,
    };

    postJSON(config, "/api/v1/learning/observations", payload, "codex observe");
  } catch (err) {
    if (process.env.QUILL_DEBUG) {
      console.error("codex observe: error:", err.message);
    }
  }
}

main();
