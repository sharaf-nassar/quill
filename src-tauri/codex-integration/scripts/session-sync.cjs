#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const os = require("os");
const { loadConfig, isLocal, postJSON } = require("./lib.cjs");

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);
    const sessionId = input.session_id || input.conversation_id || input.id;
    const transcriptPath = input.transcript_path;
    if (!sessionId || !transcriptPath) return;

    const config = loadConfig();
    if (!isLocal(config.url)) return;

    postJSON(config, "/api/v1/sessions/notify", {
      provider: "codex",
      session_id: sessionId,
      jsonl_path: transcriptPath,
      host: os.hostname(),
      cwd: input.cwd || null,
      project: input.cwd ? path.basename(input.cwd) : null,
      git_branch: input.git_branch || null,
    });
  } catch (err) {
    if (process.env.QUILL_DEBUG) {
      console.error("codex session-sync: error:", err.message);
    }
  }
}

main();
