#!/usr/bin/env node
"use strict";

const fs = require("fs");
const http = require("http");
const https = require("https");
const os = require("os");
const path = require("path");

const CHUNK_SIZE = 64 * 1024;
const MAX_TOKEN_COUNT = 100_000_000;
const LOCAL_TIMEOUT_MS = 1500;
const REMOTE_TIMEOUT_MS = 2000;
const TOKEN_FIELDS = [
  "input_tokens",
  "output_tokens",
  "cache_creation_input_tokens",
  "cache_read_input_tokens",
];

function isLocal(url) {
  return url.includes("localhost") || url.includes("127.0.0.1") || url.includes("[::1]");
}

function validatedUsage(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  if (Object.keys(value).length === 0) return null;
  const result = {};
  for (const field of TOKEN_FIELDS) {
    const count = value[field] ?? 0;
    if (!Number.isInteger(count) || count < 0 || count > MAX_TOKEN_COUNT) return null;
    result[field] = count;
  }
  return result;
}

function usageFromLine(line) {
  try {
    const record = JSON.parse(line.toString("utf8").trim());
    if (record?.type !== "assistant") return null;
    return validatedUsage(record.message?.usage);
  } catch (_) {
    return null;
  }
}

function findLastUsage(transcriptPath) {
  const fd = fs.openSync(transcriptPath, "r");
  try {
    let position = fs.fstatSync(fd).size;
    let suffix = Buffer.alloc(0);
    while (position > 0) {
      const size = Math.min(CHUNK_SIZE, position);
      position -= size;
      const chunk = Buffer.allocUnsafe(size);
      fs.readSync(fd, chunk, 0, size, position);
      const data = Buffer.concat([chunk, suffix]);
      let end = data.length;
      for (let index = data.length - 1; index >= 0; index -= 1) {
        if (data[index] !== 0x0a) continue;
        if (index + 1 < end) {
          const usage = usageFromLine(data.subarray(index + 1, end));
          if (usage) return usage;
        }
        end = index;
      }
      suffix = Buffer.from(data.subarray(0, end));
    }
    return suffix.length > 0 ? usageFromLine(suffix) : null;
  } finally {
    fs.closeSync(fd);
  }
}

function postJSON(config, payload) {
  const body = JSON.stringify(payload);
  const url = new URL(`${config.url}/api/v1/tokens`);
  const transport = url.protocol === "https:" ? https : http;
  const timeout = isLocal(config.url) ? LOCAL_TIMEOUT_MS : REMOTE_TIMEOUT_MS;
  const request = transport.request(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${config.secret}`,
      "Content-Length": Buffer.byteLength(body),
    },
  }, (response) => response.resume());
  request.on("error", () => {});
  const timer = setTimeout(() => request.destroy(), timeout);
  timer.unref?.();
  request.on("close", () => clearTimeout(timer));
  request.end(body);
}

function buildPayload(input, config, usage) {
  const payload = {
    session_id: input.session_id,
    hostname: config.hostname || os.hostname().split(".")[0] || "local",
    provider: "claude",
    ...usage,
  };
  if (input.cwd) payload.cwd = input.cwd;
  return payload;
}

function main() {
  try {
    const input = JSON.parse(fs.readFileSync(0, "utf8") || "{}");
    if (input.stop_hook_active || !input.session_id || !input.transcript_path) return;
    if (!fs.statSync(input.transcript_path).isFile()) return;

    const configPath = path.join(
      process.env.HOME || process.env.USERPROFILE || os.homedir(),
      ".config",
      "quill",
      "config.json",
    );
    const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
    if (!config.url || !config.secret) return;
    const usage = findLastUsage(input.transcript_path);
    if (!usage) return;
    postJSON(config, buildPayload(input, config, usage));
  } catch (error) {
    if (process.env.QUILL_DEBUG) console.error("report-tokens: error:", error.message);
  }
}

if (require.main === module) main();

module.exports = { buildPayload, findLastUsage, validatedUsage };
