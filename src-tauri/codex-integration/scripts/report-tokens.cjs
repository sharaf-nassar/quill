#!/usr/bin/env node
"use strict";

// Reports token usage from the latest Codex turn to the Quill widget.
// Scans the rollout transcript backward in bounded chunks so large
// transcripts are never loaded into memory and malformed trailing
// records are skipped.

const fs = require("fs");
const os = require("os");
const { loadConfig, postJSON } = require("./lib.cjs");

const CHUNK_SIZE = 64 * 1024;
const MAX_TOKEN_COUNT = 100_000_000;
const TOKEN_FIELDS = ["input_tokens", "cached_input_tokens", "output_tokens"];

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
    if (record?.type !== "event_msg") return null;
    const payload = record.payload;
    if (!payload || payload.type !== "token_count") return null;
    const info = payload.info;
    if (!info || typeof info !== "object") return null;
    return (
      validatedUsage(info.last_token_usage) ??
      validatedUsage(info.total_token_usage)
    );
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

function buildPayload(input, config, usage) {
  const payload = {
    provider: "codex",
    session_id: input.session_id,
    hostname: config.hostname || os.hostname().split(".")[0] || "local",
    input_tokens: usage.input_tokens,
    output_tokens: usage.output_tokens,
    cache_creation_input_tokens: 0,
    cache_read_input_tokens: usage.cached_input_tokens,
  };
  if (input.cwd) payload.cwd = input.cwd;
  return payload;
}

function main() {
  try {
    const input = JSON.parse(fs.readFileSync(0, "utf8") || "{}");
    if (input.stop_hook_active || !input.session_id || !input.transcript_path) return;
    if (!fs.statSync(input.transcript_path).isFile()) return;

    const config = loadConfig();
    if (!config.url || !config.secret) return;
    const usage = findLastUsage(input.transcript_path);
    if (!usage) return;
    postJSON(config, "/api/v1/tokens", buildPayload(input, config, usage), "codex report-tokens");
  } catch (error) {
    if (process.env.QUILL_DEBUG) console.error("codex report-tokens: error:", error.message);
  }
}

if (require.main === module) main();

module.exports = { buildPayload, findLastUsage, usageFromLine, validatedUsage };
