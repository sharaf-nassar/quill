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
      console.error(`session-sync: server returned ${res.statusCode}`);
    }
    res.resume();
  });

  req.on("error", (err) => {
    if (process.env.QUILL_DEBUG) console.error("session-sync: request error:", err.message);
  });
  req.end(body);
}

function extractMessages(lines) {
  const messages = [];

  for (const line of lines) {
    let entry;
    try {
      entry = JSON.parse(line);
    } catch (_) {
      continue;
    }

    if (!entry.type || entry.type === "system") continue;

    // User messages
    if (entry.type === "human" || entry.type === "user") {
      const content = entry.message?.content;
      if (!content) continue;

      if (typeof content === "string") {
        messages.push({ role: "user", text: content });
      } else if (Array.isArray(content)) {
        const textParts = content
          .filter((b) => b.type === "text" && !b.isMeta)
          .map((b) => b.text)
          .filter(Boolean);
        if (textParts.length > 0) {
          messages.push({ role: "user", text: textParts.join("\n") });
        }
      }
      continue;
    }

    // Assistant messages
    if (entry.type === "assistant") {
      const content = entry.message?.content;
      if (!content || !Array.isArray(content)) continue;

      const textParts = content
        .filter((b) => {
          if (b.type !== "text") return false;
          if (b.isMeta) return false;
          return true;
        })
        .map((b) => b.text)
        .filter(Boolean);

      if (textParts.length > 0) {
        messages.push({ role: "assistant", text: textParts.join("\n") });
      }
      continue;
    }
  }

  return messages;
}

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);

    if (input.stop_hook_active) return;

    const sessionId = input.session_id;
    const transcriptPath = input.transcript_path;
    if (!sessionId || !transcriptPath) return;

    const configPath = path.join(
      process.env.HOME || process.env.USERPROFILE,
      ".config",
      "quill",
      "config.json"
    );
    const config = JSON.parse(fs.readFileSync(configPath, "utf8"));

    if (isLocal(config.url)) {
      // LOCAL: notify the server to read the JSONL itself
      postJSON(config, "/api/v1/sessions/notify", {
        session_id: sessionId,
        jsonl_path: transcriptPath,
      });
      return;
    }

    // REMOTE: read JSONL, extract new messages, send them
    const trackingFile = path.join(os.tmpdir(), `.quill-sync-${sessionId}`);

    let lastSent = 0;
    try {
      lastSent = parseInt(fs.readFileSync(trackingFile, "utf8"), 10) || 0;
    } catch (_) {
      // No tracking file yet
    }

    let content;
    try {
      content = fs.readFileSync(transcriptPath, "utf8");
    } catch (_) {
      return;
    }

    const allLines = content.split("\n").filter((l) => l.trim().length > 0);
    if (allLines.length <= lastSent) return;

    const newLines = allLines.slice(lastSent);
    const messages = extractMessages(newLines);

    if (messages.length === 0) {
      // Update tracking even if no extractable messages, to skip these lines next time
      fs.writeFileSync(trackingFile, String(allLines.length));
      return;
    }

    postJSON(config, "/api/v1/sessions/messages", {
      host: os.hostname(),
      session_id: sessionId,
      project: path.basename(input.cwd || ""),
      git_branch: input.git_branch || null,
      messages,
    });

    fs.writeFileSync(trackingFile, String(allLines.length));
  } catch (err) {
    if (process.env.QUILL_DEBUG) console.error("session-sync: error:", err.message);
  }
}

main();
