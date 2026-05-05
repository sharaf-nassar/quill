#!/usr/bin/env node
"use strict";

const crypto = require("crypto");
const fs = require("fs");
const http = require("http");
const https = require("https");
const os = require("os");
const path = require("path");

const SCHEMA_VERSION = 1;
const ENDPOINT = "/api/v1/context-savings/events";
const LOCAL_TIMEOUT_MS = 1000;
const REMOTE_TIMEOUT_MS = 1500;

function homeDir() {
  return process.env.HOME || process.env.USERPROFILE || os.homedir();
}

function configPath() {
  return path.join(homeDir(), ".config", "quill", "config.json");
}

function readConfig() {
  try {
    const config = JSON.parse(fs.readFileSync(configPath(), "utf8"));
    if (!config.url || !config.secret) return null;
    return config;
  } catch (_) {
    return null;
  }
}

function isLocal(urlStr) {
  return urlStr.includes("localhost") || urlStr.includes("127.0.0.1") || urlStr.includes("[::1]");
}

function inferProvider(input = {}) {
  if (input.provider) return String(input.provider);
  if (process.env.QUILL_PROVIDER) return process.env.QUILL_PROVIDER;
  return __dirname.includes("codex") ? "codex" : "claude";
}

function inferSessionId(input = {}) {
  return input.session_id ||
    input.conversation_id ||
    input.id ||
    process.env.QUILL_SESSION_ID ||
    process.env.CLAUDE_SESSION_ID ||
    process.env.CODEX_SESSION_ID ||
    null;
}

function byteLength(value) {
  if (value === undefined || value === null) return 0;
  if (Buffer.isBuffer(value)) return value.length;
  if (typeof value === "string") return Buffer.byteLength(value, "utf8");
  return Buffer.byteLength(JSON.stringify(value), "utf8");
}

function nullableInteger(value) {
  if (value === undefined || value === null) return null;
  const number = Number(value);
  return Number.isFinite(number) ? Math.max(0, Math.trunc(number)) : null;
}

function tokensFromBytes(value) {
  const bytes = nullableInteger(value);
  return bytes === null ? 0 : Math.ceil(bytes / 4);
}

function stableEventId(event) {
  return `ctx_${crypto.createHash("sha256").update(JSON.stringify(event)).digest("hex").slice(0, 32)}`;
}

// Canonical taxonomy: keep in sync with src-tauri/src/context_category.rs
// and src-tauri/claude-integration/mcp/tools/context.py.
function deriveCategory(eventType, decision) {
  switch (eventType) {
    case "mcp.index":
    case "mcp.fetch":
      return "preservation";
    case "mcp.execute":
      return decision === "indexed" ? "preservation" : "routing";
    case "mcp.search":
      return "routing";
    case "mcp.source_read":
      return "retrieval";
    case "mcp.snapshot":
      return decision === "created" ? "preservation" : "retrieval";
    case "mcp.continuity":
      return "telemetry";
    case "router.guidance":
    case "router.denial":
      return "routing";
    case "capture.event":
    case "capture.snapshot":
      return "telemetry";
    case "capture.guidance":
      return "routing";
    default:
      return "unknown";
  }
}

function buildContextSavingsEvent(input, fields) {
  const config = readConfig();
  const indexedBytes = nullableInteger(fields.indexedBytes);
  const returnedBytes = nullableInteger(fields.returnedBytes);
  const inputBytes = nullableInteger(fields.inputBytes);
  const hasByteEstimate = indexedBytes !== null || returnedBytes !== null || inputBytes !== null;
  const category = fields.category || deriveCategory(fields.eventType, fields.decision || "recorded");
  // Only preservation/retrieval events default tokensSaved/tokensPreserved from indexedBytes.
  // Routing and telemetry events default to 0 unless the caller passes explicit values, so
  // hook payloads (capture.event, router.guidance, etc.) no longer inflate the savings metric.
  const tokenScope = category === "preservation" || category === "retrieval";
  const savedBaseline = indexedBytes !== null ? indexedBytes : inputBytes;
  const savedBytes = tokenScope && savedBaseline !== null
    ? Math.max(0, savedBaseline - (returnedBytes ?? 0))
    : null;

  const event = {
    eventId: "",
    schemaVersion: SCHEMA_VERSION,
    provider: fields.provider || inferProvider(input),
    sessionId: fields.sessionId || inferSessionId(input),
    hostname: fields.hostname || config?.hostname || os.hostname(),
    cwd: fields.cwd || input.cwd || process.cwd(),
    timestamp: fields.timestamp || new Date().toISOString(),
    eventType: fields.eventType,
    source: fields.source || "context",
    decision: fields.decision || "recorded",
    category,
    reason: fields.reason || null,
    delivered: fields.delivered === undefined ? true : Boolean(fields.delivered),
    indexedBytes,
    returnedBytes,
    inputBytes,
    tokensIndexedEst: fields.tokensIndexedEst ?? tokensFromBytes(indexedBytes),
    tokensReturnedEst: fields.tokensReturnedEst ?? tokensFromBytes(returnedBytes),
    tokensSavedEst: fields.tokensSavedEst ?? (tokenScope ? tokensFromBytes(savedBytes) : 0),
    tokensPreservedEst: fields.tokensPreservedEst ?? (tokenScope ? tokensFromBytes(indexedBytes) : 0),
    estimateMethod: fields.estimateMethod || (hasByteEstimate ? "ceil_bytes_div_4" : "none"),
    estimateConfidence: fields.estimateConfidence ?? (hasByteEstimate ? 1 : 0),
    sourceRef: fields.sourceRef || null,
    snapshotRef: fields.snapshotRef || null,
    metadata: fields.metadata || {},
  };
  event.eventId = fields.eventId || stableEventId(event);
  return event;
}

function postContextSavingsEvents(events, label = "context-telemetry") {
  const cleanEvents = (events || []).filter(Boolean);
  if (cleanEvents.length === 0) return;

  const config = readConfig();
  if (!config) return;

  let url;
  try {
    url = new URL(`${String(config.url).replace(/\/+$/, "")}${ENDPOINT}`);
  } catch (_) {
    return;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return;

  const body = JSON.stringify({ events: cleanEvents });
  const mod = url.protocol === "https:" ? https : http;
  const timeoutMs = isLocal(config.url) ? LOCAL_TIMEOUT_MS : REMOTE_TIMEOUT_MS;

  let settled = false;
  let timer;
  const clearTimer = () => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
  };

  let req;
  try {
    req = mod.request(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${config.secret}`,
        "Content-Length": Buffer.byteLength(body),
      },
    }, (res) => {
      clearTimer();
      if (res.statusCode >= 400 && process.env.QUILL_DEBUG) {
        console.error(`${label}: context savings server returned ${res.statusCode}`);
      }
      res.resume();
    });
  } catch (err) {
    if (process.env.QUILL_DEBUG) {
      console.error(`${label}: context savings request setup error:`, err.message);
    }
    return;
  }

  req.on("error", (err) => {
    clearTimer();
    if (process.env.QUILL_DEBUG) {
      console.error(`${label}: context savings request error:`, err.message);
    }
  });
  req.on("close", clearTimer);
  timer = setTimeout(() => {
    req.destroy(new Error(`timed out after ${timeoutMs}ms`));
  }, timeoutMs);
  timer.unref?.();
  req.end(body);
}

module.exports = {
  buildContextSavingsEvent,
  byteLength,
  deriveCategory,
  postContextSavingsEvents,
  tokensFromBytes,
};
