#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const https = require("https");
const http = require("http");

async function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);

    if (input.stop_hook_active) return;

    const configPath = path.join(
      process.env.HOME || process.env.USERPROFILE,
      ".config",
      "quill",
      "config.json"
    );
    const config = JSON.parse(fs.readFileSync(configPath, "utf8"));

    const payload = JSON.stringify({
      session_id: input.session_id,
      transcript_path: input.transcript_path,
      cwd: input.cwd,
    });

    const url = new URL(`${config.url}/api/v1/learning/session-end`);
    const mod = url.protocol === "https:" ? https : http;

    const req = mod.request(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${config.secret}`,
        "Content-Length": Buffer.byteLength(payload),
      },
    });

    req.on("error", (err) => {
      if (process.env.QUILL_DEBUG) console.error("session-end-learn: request error:", err.message);
    });
    req.write(payload);
    req.end();
  } catch (err) {
    if (process.env.QUILL_DEBUG) console.error("session-end-learn: error:", err.message);
  }
}

main();
