#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const os = require("os");
const https = require("https");
const http = require("http");

function isLocal(urlStr) {
  return urlStr.includes("localhost") || urlStr.includes("127.0.0.1");
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

function postJSON(config, endpoint, payload) {
  const body = JSON.stringify(payload);
  const url = new URL(`${config.url}${endpoint}`);
  const mod = url.protocol === "https:" ? https : http;

  const req = mod.request(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${config.secret}`,
      "Content-Length": Buffer.byteLength(body),
    },
    timeout: 3000,
  }, (res) => {
    if (res.statusCode >= 400 && process.env.QUILL_DEBUG) {
      console.error(`codex session-sync: server returned ${res.statusCode}`);
    }
    res.resume();
  });

  req.on("error", (err) => {
    if (process.env.QUILL_DEBUG) {
      console.error("codex session-sync: request error:", err.message);
    }
  });
  req.end(body);
}

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);
    const sessionId = input.session_id;
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
