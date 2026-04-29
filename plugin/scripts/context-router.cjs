#!/usr/bin/env node
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const {
  buildContextSavingsEvent,
  byteLength,
  postContextSavingsEvents,
} = require("./context-telemetry.cjs");

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

function homeDir() {
  return process.env.HOME || process.env.USERPROFILE || os.homedir();
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
      "Quill context routing blocked WebFetch because full page dumps can exhaust context. Use mcp__quill__quill_fetch_and_index for web content, then mcp__quill__quill_search_context to retrieve focused results.",
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
        "Quill context routing blocked a raw network fetch that can dump large content. Use mcp__quill__quill_fetch_and_index or fetch to a file quietly and summarize; do not stream full pages into the transcript.",
        { route: "raw-network-fetch", commandBytes: byteLength(command) },
      );
    }

    if (isLargeBuildCommand(command) && once(provider, sessionId, "build")) {
      return additionalContext(input, provider, sessionId, tool, guidance("build"), "build");
    }

    if (isLikelyVerboseBash(command) && once(provider, sessionId, "bash")) {
      return additionalContext(input, provider, sessionId, tool, guidance("bash"), "bash");
    }

    return null;
  }

  if (tool === "Read" && once(provider, sessionId, "read")) {
    return additionalContext(input, provider, sessionId, tool, guidance("read"), "read");
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

module.exports = { route };
