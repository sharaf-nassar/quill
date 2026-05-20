#!/usr/bin/env node
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
let telemetry = {};
try {
  telemetry = require("./context-telemetry.cjs");
} catch (_) {
  telemetry = {};
}

const buildContextSavingsEvent = telemetry.buildContextSavingsEvent || (() => null);
const postContextSavingsEvents = telemetry.postContextSavingsEvents || (() => {});
const byteLength = telemetry.byteLength || fallbackByteLength;

const TOOL_ALIASES = {
  shell: "Bash",
  shell_command: "Bash",
  exec_command: "Bash",
  "container.exec": "Bash",
  local_shell: "Bash",
  run_shell_command: "Bash",
  run_in_terminal: "Bash",
  grep_files: "Grep",
  grep_search: "Grep",
  search_file_content: "Grep",
  read_file: "Read",
  read_many_files: "Read",
  view: "Read",
  fetch: "WebFetch",
  web_fetch: "WebFetch",
};

const CONTEXT_TOOLS = [
  "mcp__quill__quill_search_context",
  "mcp__quill__quill_execute",
  "mcp__quill__quill_execute_file",
  "mcp__quill__quill_batch_execute",
  "mcp__quill__quill_fetch_and_index",
  "mcp__quill__search_history",
];

const MARKER_RETENTION_MS = 30 * 24 * 60 * 60 * 1000;
const CLEANUP_INTERVAL_MS = 24 * 60 * 60 * 1000;
const TAINTED_STATE_FILE = "tainted.json";
const TAINTED_MAX_PATHS = 256;

// Pure-reader commands that dump file content into the transcript when given a
// path argument. Intentionally excludes interpreters (python/node/ruby/perl)
// because those usually execute rather than print; including them would block
// legitimate `bash /tmp/installer.sh` style fetch-and-run flows.
const READER_COMMAND_PATTERN =
  /\b(cat|bat|head|tail|less|more|view|od|xxd|strings|hexdump|sed|awk|grep|rg|ack|jq|yq|xq|xmllint)\b/i;

function homeDir() {
  return process.env.HOME || process.env.USERPROFILE || os.homedir();
}

function fallbackByteLength(value) {
  if (value === undefined || value === null) return 0;
  if (Buffer.isBuffer(value)) return value.length;
  if (typeof value === "string") return Buffer.byteLength(value, "utf8");
  return Buffer.byteLength(JSON.stringify(value), "utf8");
}

function inferProvider(input) {
  if (input.provider) return String(input.provider);
  if (process.env.QUILL_PROVIDER) return process.env.QUILL_PROVIDER;
  return __dirname.includes("codex") ? "codex" : "claude";
}

function safeName(value) {
  return String(value || "unknown").replace(/[^a-zA-Z0-9._-]+/g, "_").slice(0, 120);
}

function markerDir(provider, sessionId) {
  return path.join(
    homeDir(),
    ".config",
    "quill",
    "context",
    "markers",
    `${safeName(provider)}-${safeName(sessionId || process.ppid)}`,
  );
}

function markerRoot() {
  return path.join(homeDir(), ".config", "quill", "context", "markers");
}

function markerCleanupStatePath() {
  return path.join(markerRoot(), ".cleanup-state.json");
}

function shouldCleanupMarkers() {
  const statePath = markerCleanupStatePath();
  try {
    const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
    const lastCleanup = Date.parse(state.lastCleanup || 0);
    return !Number.isFinite(lastCleanup) || Date.now() - lastCleanup > CLEANUP_INTERVAL_MS;
  } catch (_) {
    return true;
  }
}

function writeMarkerCleanupState() {
  const statePath = markerCleanupStatePath();
  fs.mkdirSync(path.dirname(statePath), { recursive: true });
  fs.writeFileSync(statePath, JSON.stringify({ lastCleanup: new Date().toISOString() }), "utf8");
}

function maybeCleanupMarkers() {
  if (!shouldCleanupMarkers()) return;
  const root = markerRoot();
  const cutoffMs = Date.now() - MARKER_RETENTION_MS;
  try {
    if (fs.existsSync(root)) {
      for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
        if (!entry.isDirectory()) continue;
        const entryPath = path.join(root, entry.name);
        const stat = fs.statSync(entryPath);
        if (stat.mtimeMs < cutoffMs) fs.rmSync(entryPath, { recursive: true, force: true });
      }
    }
    writeMarkerCleanupState();
  } catch (_) {
    // Cleanup is opportunistic; routing must keep working.
  }
}

function once(provider, sessionId, key) {
  const dir = markerDir(provider, sessionId);
  try {
    fs.mkdirSync(dir, { recursive: true });
    const fd = fs.openSync(path.join(dir, safeName(key)), "wx");
    fs.closeSync(fd);
    return true;
  } catch (_) {
    return false;
  }
}

function stripHeredocs(command) {
  return command.replace(/<<-?\s*["']?([A-Za-z0-9_]+)["']?[\s\S]*?\n\s*\1/g, "");
}

function stripQuotedContent(command) {
  return stripHeredocs(command)
    .replace(/'[^']*'/g, "''")
    .replace(/"[^"]*"/g, '""');
}

function commandFromInput(toolInput) {
  if (!toolInput) return "";
  if (typeof toolInput === "string") return toolInput;
  return String(toolInput.command || toolInput.cmd || toolInput.script || "");
}

function hasCurlSilentFlag(segment) {
  return /(^|\s)-[A-Za-z]*s[A-Za-z]*(\s|$)/.test(segment) || /\s--silent(\s|$)/.test(segment);
}

function hasWgetQuietFlag(segment) {
  return /(^|\s)-[A-Za-z]*q[A-Za-z]*(\s|$)/.test(segment) || /\s--quiet(\s|$)/.test(segment);
}

function hasCurlFileOutput(segment) {
  return /\s(-o|--output)\s+\S+/.test(segment) || /\s(-O|--remote-name)(\s|$)/.test(segment) || /\s>>?\s*\S+/.test(segment);
}

function hasWgetFileOutput(segment) {
  return /\s(-O|--output-document)\s+\S+/.test(segment) || /\s>>?\s*\S+/.test(segment);
}

function isStdoutTarget(segment) {
  return /\s(-o|--output|-O|--output-document)\s+(-|\/dev\/stdout)(\s|$)/.test(segment);
}

function hasRawNetworkDump(command) {
  const stripped = stripQuotedContent(command);
  if (!/(^|\s|&&|\|\||;)(curl|wget)\s/i.test(stripped)) return false;

  return stripped.split(/\s*(?:&&|\|\||;)\s*/).some((segment) => {
    const s = segment.trim();
    if (!/(^|\s)(curl|wget)\s/i.test(s)) return false;
    if (/\s(-I|--head)(\s|$)/.test(s)) return false;

    const isCurl = /\bcurl\b/i.test(s);
    const hasFileOutput = isCurl ? hasCurlFileOutput(s) : hasWgetFileOutput(s);
    const isQuiet = isCurl ? hasCurlSilentFlag(s) : hasWgetQuietFlag(s);
    const isVerbose = /\s(-v|--verbose|--trace|--trace-ascii|-D\s+-)(\s|$)/.test(s);

    return !hasFileOutput || !isQuiet || isVerbose || isStdoutTarget(s);
  });
}

function isInlineNetworkFetch(command) {
  const visible = stripHeredocs(command);
  return /fetch\s*\(\s*["']https?:\/\//i.test(visible) ||
    /requests\.(get|post|put|patch)\s*\(/i.test(visible) ||
    /http\.(get|request)\s*\(/i.test(visible);
}

function isLargeBuildCommand(command) {
  const stripped = stripQuotedContent(command);
  return /(^|\s)(npm|pnpm|yarn|bun)\s+(run\s+)?(build|test|lint|check)\b/i.test(stripped) ||
    /(^|\s)(cargo)\s+(build|test|clippy)\b/i.test(stripped) ||
    /(^|\s)(go)\s+test\s+\.\/\.\./i.test(stripped) ||
    /(^|\s)(\.\/gradlew|gradlew|gradle|\.\/mvnw|mvnw|mvn|make|cmake)\b/i.test(stripped) ||
    /(^|\s)docker\s+(build|compose)\b/i.test(stripped);
}

function isLikelyVerboseBash(command) {
  const stripped = stripQuotedContent(command);
  return /\b(git\s+(diff|show|log)|rg|grep|find|tree|ls\s+-R|cat|sed|awk|pytest|journalctl|docker\s+logs|kubectl\s+logs|tail\s+-f)\b/i.test(stripped) ||
    stripped.length > 220;
}

// --- Taint tracking ---------------------------------------------------------
//
// When curl/wget quietly writes to a file (`-o PATH`, `-O`, `--output-document`,
// or `>`/`>>` redirect), record the destination so we can deny later reads of
// that path. Without this, the model bypasses the network-fetch guard by
// splitting `curl ... | jq .` into `curl -o /tmp/x` followed by `jq . /tmp/x`,
// which dumps the same response into the transcript anyway.

function taintedStatePath(provider, sessionId) {
  return path.join(markerDir(provider, sessionId), TAINTED_STATE_FILE);
}

function loadTainted(provider, sessionId) {
  try {
    const raw = JSON.parse(fs.readFileSync(taintedStatePath(provider, sessionId), "utf8"));
    return new Set(Array.isArray(raw.paths) ? raw.paths : []);
  } catch (_) {
    return new Set();
  }
}

function saveTainted(provider, sessionId, set) {
  try {
    const filePath = taintedStatePath(provider, sessionId);
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    const arr = Array.from(set);
    const trimmed = arr.length > TAINTED_MAX_PATHS ? arr.slice(-TAINTED_MAX_PATHS) : arr;
    fs.writeFileSync(filePath, JSON.stringify({ paths: trimmed }), "utf8");
  } catch (_) {
    // best-effort; routing must keep working
  }
}

function resolveLiteralPath(p) {
  if (!p) return p;
  let out = p;
  if (out.startsWith("~")) out = path.join(homeDir(), out.slice(1));
  if (!path.isAbsolute(out)) {
    try {
      out = path.resolve(process.cwd(), out);
    } catch (_) {
      return p;
    }
  }
  return out;
}

function extractFetchOutputPaths(command) {
  const stripped = stripQuotedContent(command);
  const out = [];
  for (const segment of stripped.split(/\s*(?:&&|\|\||;)\s*/)) {
    if (!/(^|\s)(curl|wget)\s/i.test(segment)) continue;
    if (/\s(-I|--head)(\s|$)/.test(segment)) continue;
    for (const m of segment.matchAll(/\s(?:-o|--output|-O|--output-document)\s+(\S+)/g)) {
      const p = m[1];
      if (p && p !== "-" && p !== "/dev/stdout" && p !== "/dev/null") out.push(p);
    }
    for (const m of segment.matchAll(/\s>>?\s*(\S+)/g)) {
      const p = m[1];
      if (p && p !== "/dev/null" && p !== "/dev/stdout") out.push(p);
    }
  }
  return out;
}

function recordTainted(provider, sessionId, paths) {
  if (!paths || paths.length === 0) return;
  const set = loadTainted(provider, sessionId);
  for (const p of paths) {
    set.add(p);
    const resolved = resolveLiteralPath(p);
    if (resolved && resolved !== p) set.add(resolved);
  }
  saveTainted(provider, sessionId, set);
}

function escapeRegExp(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function commandReadsTaintedPath(command, taintedSet) {
  if (!command || taintedSet.size === 0) return null;
  const stripped = stripQuotedContent(command);
  if (!READER_COMMAND_PATTERN.test(stripped)) return null;
  for (const p of taintedSet) {
    const esc = escapeRegExp(p);
    if (new RegExp(`(?:^|[\\s=])${esc}(?:[\\s)>;|&]|$)`).test(stripped)) return p;
  }
  return null;
}

function readTargetsTaintedPath(filePath, taintedSet) {
  if (!filePath || taintedSet.size === 0) return null;
  if (taintedSet.has(filePath)) return filePath;
  const resolved = resolveLiteralPath(filePath);
  if (resolved && taintedSet.has(resolved)) return resolved;
  return null;
}

// --- Routing ----------------------------------------------------------------

function guidance(type) {
  const tools = CONTEXT_TOOLS.join(", ");
  if (type === "read") {
    return `Quill context: Read is best for files you will edit. For prior session context or raw transcript details, use ${tools} instead of dumping large files.`;
  }
  if (type === "grep") {
    return `Quill context: broad Grep output can crowd the conversation. For past work, use ${tools}; for code search, summarize matches instead of pasting long output.`;
  }
  if (type === "build") {
    return `Quill context: build/test commands can produce large logs. Capture logs to a file or tail failures only, and use ${tools} for prior debug context.`;
  }
  return `Quill context: keep Bash output short. For session history, prior work, or transcript details, prefer ${tools}; summarize broad shell output.`;
}

function fetchDenyReason() {
  return [
    "Quill context routing blocked a raw network fetch.",
    "DO NOT bypass by fetching to a file and then reading it back (`curl -o X && cat X`, jq, grep, sed, awk, Read, etc.) — that defeats this guard and the path will be denied on the next read.",
    "Use instead:",
    "  - mcp__quill__quill_execute for `curl ... | jq` workflows (output is bounded + indexed automatically)",
    "  - mcp__quill__quill_fetch_and_index for HTML / docs / web pages",
    "  - mcp__quill__quill_search_context to retrieve focused chunks afterwards",
    "Only use `curl -o path` / `wget -O path` for binary artifacts you will EXECUTE or INSTALL (tarballs, packages, images) — never to inspect content.",
  ].join("\n");
}

function taintedReadDenyReason(tool, taintedPath) {
  return [
    `Quill context routing blocked ${tool} on ${taintedPath} because that path was written by an earlier curl/wget in this session.`,
    "Reading freshly-fetched network content into the transcript defeats the fetch routing guard.",
    "Use mcp__quill__quill_search_context if the response was already indexed, or mcp__quill__quill_execute to re-fetch with bounded output (e.g. `curl -sS URL | jq ...`).",
    "If this file is genuinely not network content (you reused the path for a scratch artifact), choose a different filename for the fetch and try again.",
  ].join("\n");
}

function emitRouterTelemetry(input, fields) {
  postContextSavingsEvents([
    buildContextSavingsEvent(input, {
      source: "context-router",
      provider: fields.provider,
      sessionId: String(fields.sessionId),
      eventType: fields.eventType,
      decision: fields.decision,
      reason: fields.reason,
      delivered: fields.delivered,
      returnedBytes: fields.returnedText ? byteLength(fields.returnedText) : null,
      tokensSavedEst: 0,
      tokensPreservedEst: 0,
      metadata: {
        eventCount: 1,
        toolName: fields.tool,
        ...fields.metadata,
      },
    }),
  ], "context-router");
}

function deny(input, provider, sessionId, tool, reason, metadata = {}) {
  emitRouterTelemetry(input, {
    provider,
    sessionId,
    tool,
    eventType: "router.denial",
    decision: "deny",
    reason,
    delivered: true,
    returnedText: reason,
    metadata,
  });
  return {
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: reason,
    },
  };
}

function additionalContext(input, provider, sessionId, tool, message, guidanceType) {
  const delivered = provider !== "codex";
  emitRouterTelemetry(input, {
    provider,
    sessionId,
    tool,
    eventType: "router.guidance",
    decision: "guide",
    reason: guidanceType,
    delivered,
    returnedText: delivered ? message : null,
    metadata: {
      guidanceType,
    },
  });
  if (provider === "codex") return null;
  return {
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      additionalContext: message,
    },
  };
}

function route(input) {
  if (!input || input.hook_event_name !== "PreToolUse") return null;
  maybeCleanupMarkers();

  const provider = inferProvider(input);
  const sessionId = input.session_id || input.conversation_id || input.id || process.ppid;
  const tool = TOOL_ALIASES[input.tool_name] || input.tool_name || "";
  const toolInput = input.tool_input || {};

  if (tool === "WebFetch") {
    return deny(
      input,
      provider,
      sessionId,
      tool,
      "Quill context routing blocked WebFetch because full page dumps can exhaust context. Use mcp__quill__quill_fetch_and_index for web content, then mcp__quill__quill_search_context to retrieve focused chunks.",
      { route: "webfetch" },
    );
  }

  if (tool === "Bash") {
    const command = commandFromInput(toolInput);
    if (!command) return null;

    if (hasRawNetworkDump(command) || isInlineNetworkFetch(command)) {
      return deny(
        input,
        provider,
        sessionId,
        tool,
        fetchDenyReason(),
        { route: "raw-network-fetch", commandBytes: byteLength(command) },
      );
    }

    const tainted = loadTainted(provider, sessionId);
    if (tainted.size > 0) {
      const hit = commandReadsTaintedPath(command, tainted);
      if (hit) {
        return deny(
          input,
          provider,
          sessionId,
          tool,
          taintedReadDenyReason("Bash", hit),
          { route: "tainted-read-bash", path: hit },
        );
      }
    }

    const outputs = extractFetchOutputPaths(command);
    if (outputs.length > 0) recordTainted(provider, sessionId, outputs);

    if (isLargeBuildCommand(command) && once(provider, sessionId, "build")) {
      return additionalContext(input, provider, sessionId, tool, guidance("build"), "build");
    }

    if (isLikelyVerboseBash(command) && once(provider, sessionId, "bash")) {
      return additionalContext(input, provider, sessionId, tool, guidance("bash"), "bash");
    }

    return null;
  }

  if (tool === "Read") {
    const tainted = loadTainted(provider, sessionId);
    if (tainted.size > 0) {
      const hit = readTargetsTaintedPath(String(toolInput.file_path || ""), tainted);
      if (hit) {
        return deny(
          input,
          provider,
          sessionId,
          tool,
          taintedReadDenyReason("Read", hit),
          { route: "tainted-read", path: hit },
        );
      }
    }
    if (once(provider, sessionId, "read")) {
      return additionalContext(input, provider, sessionId, tool, guidance("read"), "read");
    }
    return null;
  }

  if (tool === "Grep" && once(provider, sessionId, "grep")) {
    return additionalContext(input, provider, sessionId, tool, guidance("grep"), "grep");
  }

  return null;
}

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw || "{}");
    const response = route(input);
    if (response) process.stdout.write(`${JSON.stringify(response)}\n`);
  } catch (err) {
    if (process.env.QUILL_DEBUG) console.error("context-router: error:", err.message);
  }
}

if (require.main === module) {
  main();
}

module.exports = {
  route,
  hasRawNetworkDump,
  isInlineNetworkFetch,
  extractFetchOutputPaths,
  commandReadsTaintedPath,
  readTargetsTaintedPath,
  loadTainted,
  recordTainted,
  fetchDenyReason,
  taintedReadDenyReason,
};
