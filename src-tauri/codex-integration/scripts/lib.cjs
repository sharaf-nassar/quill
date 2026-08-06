"use strict";

// Shared helpers for the Quill-managed Codex hook scripts. Staged to
// ~/.config/quill/codex/scripts/ alongside the scripts that require it.

const fs = require("fs");
const path = require("path");

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

// Fire-and-forget POST. Never rejects; hook scripts must exit 0 regardless
// of delivery. Local targets get a shorter timeout than remote ones.
function postJSON(config, endpoint, payload, label) {
  const timeoutMs = isLocal(config.url) ? LOCAL_TIMEOUT_MS : REMOTE_TIMEOUT_MS;
  return fetch(`${config.url}${endpoint}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${config.secret}`,
    },
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(timeoutMs),
  }).then(
    (res) => {
      if (res.status >= 400 && process.env.QUILL_DEBUG) {
        console.error(`${label}: server returned ${res.status}`);
      }
      return res.body?.cancel().catch(() => {});
    },
    (err) => {
      if (process.env.QUILL_DEBUG) {
        console.error(`${label}: request error:`, err.message);
      }
    },
  );
}

module.exports = { loadConfig, isLocal, postJSON };
