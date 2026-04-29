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

const RECENT_WINDOW_MS = 7 * 24 * 60 * 60 * 1000;
const MAX_READ_BYTES = 256 * 1024;
const CONTEXT_TOOLS = [
  "mcp__quill__quill_search_context",
  "mcp__quill__quill_get_context_source",
  "mcp__quill__quill_record_continuity_event",
  "mcp__quill__quill_create_compaction_snapshot",
  "mcp__quill__quill_get_compaction_snapshot",
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

function continuityDir() {
  return path.join(homeDir(), ".config", "quill", "context", "continuity");
}

function safeName(value) {
  return String(value || "unknown").replace(/[^a-zA-Z0-9._-]+/g, "_").slice(0, 120);
}

function eventName(input) {
  return input.hook_event_name || input.hookEventName || input.event || "unknown";
}

function sessionId(input) {
  return input.session_id || input.conversation_id || input.id || `pid-${process.ppid}`;
}

function compactText(value, maxLen) {
  if (value === undefined || value === null) return null;
  const text = String(value).replace(/\s+/g, " ").trim();
  if (!text) return null;
  return text.length > maxLen ? `${text.slice(0, maxLen - 3)}...` : text;
}

function promptText(input) {
  return input.prompt ||
    input.message ||
    input.user_prompt ||
    input.tool_input?.prompt ||
    input.tool_input?.message ||
    null;
}

function shouldSkipPrompt(text) {
  const trimmed = String(text || "").trim();
  return trimmed.startsWith("<task-notification>") ||
    trimmed.startsWith("<system-reminder>") ||
    trimmed.startsWith("<context_guidance>") ||
    trimmed.startsWith("<tool-result>");
}

function unique(values, max) {
  const seen = new Set();
  const out = [];
  for (const value of values) {
    const item = compactText(value, 180);
    if (!item || seen.has(item)) continue;
    seen.add(item);
    out.push(item);
    if (out.length >= max) break;
  }
  return out;
}

function splitSentences(text) {
  return String(text || "")
    .split(/(?<=[.!?])\s+|\n+/)
    .map((part) => compactText(part, 180))
    .filter(Boolean);
}

function extractHints(summary) {
  const sentences = splitSentences(summary);
  const decisions = sentences.filter((s) =>
    /\b(do not|don't|never|always|prefer|instead|use|choose|approved|looks good|sounds good|yes|no)\b/i.test(s),
  );
  const tasks = sentences.filter((s) =>
    /\b(add|build|create|fix|update|implement|wire|investigate|debug|review|finish|continue|preserve)\b/i.test(s),
  );

  let intent = null;
  if (/\b(debug|investigate|root cause|error)\b/i.test(summary)) intent = "debug";
  else if (/\b(review|audit|check)\b/i.test(summary)) intent = "review";
  else if (/\b(add|build|create|fix|update|implement|wire)\b/i.test(summary)) intent = "implement";
  else if (/\b(plan|discuss|think through|brainstorm)\b/i.test(summary)) intent = "discuss";

  return {
    intent,
    decisions: unique(decisions, 3),
    tasks: unique(tasks, 3),
  };
}

function appendJsonLine(filePath, record) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.appendFileSync(filePath, `${JSON.stringify(record)}\n`, "utf8");
}

function jsonLineBytes(record) {
  return byteLength(`${JSON.stringify(record)}\n`);
}

function readTail(filePath) {
  try {
    const stat = fs.statSync(filePath);
    const start = Math.max(0, stat.size - MAX_READ_BYTES);
    const fd = fs.openSync(filePath, "r");
    const buffer = Buffer.alloc(stat.size - start);
    fs.readSync(fd, buffer, 0, buffer.length, start);
    fs.closeSync(fd);
    return buffer.toString("utf8");
  } catch (_) {
    return "";
  }
}

function parseRecent(filePath, sinceMs) {
  return readTail(filePath)
    .split(/\n+/)
    .filter(Boolean)
    .map((line) => {
      try {
        return JSON.parse(line);
      } catch (_) {
        return null;
      }
    })
    .filter((record) => record && Date.parse(record.timestamp || record.ts || 0) >= sinceMs);
}

function recentRecords() {
  const dir = continuityDir();
  const sinceMs = Date.now() - RECENT_WINDOW_MS;
  return [
    ...parseRecent(path.join(dir, "snapshots.jsonl"), sinceMs),
    ...parseRecent(path.join(dir, "events.jsonl"), sinceMs),
  ].sort((a, b) => Date.parse(b.timestamp || b.ts || 0) - Date.parse(a.timestamp || a.ts || 0));
}

function buildEvent(input) {
  const provider = inferProvider(input);
  const prompt = promptText(input);
  const promptSummary = prompt && !shouldSkipPrompt(prompt) ? compactText(prompt, 300) : null;
  const hints = promptSummary ? extractHints(promptSummary) : extractHints(input.last_assistant_message || "");

  return {
    kind: "event",
    timestamp: new Date().toISOString(),
    provider,
    session_id: sessionId(input),
    cwd: input.cwd || process.cwd(),
    hook_event: eventName(input),
    source: input.source || null,
    prompt_summary: promptSummary,
    assistant_summary: compactText(input.last_assistant_message, 240),
    hints,
  };
}

function buildSnapshot(input, currentEvent) {
  const records = recentRecords()
    .filter((record) => record.session_id === currentEvent.session_id)
    .slice(0, 12);
  const prompts = unique(records.map((record) => record.prompt_summary).filter(Boolean), 4);
  if (currentEvent.prompt_summary) prompts.unshift(currentEvent.prompt_summary);

  return {
    kind: "snapshot",
    timestamp: new Date().toISOString(),
    provider: currentEvent.provider,
    session_id: currentEvent.session_id,
    cwd: currentEvent.cwd,
    hook_event: currentEvent.hook_event,
    prompt_summaries: unique(prompts, 5),
    decisions: unique(records.flatMap((record) => record.hints?.decisions || []), 5),
    tasks: unique(records.flatMap((record) => record.hints?.tasks || []), 5),
    intent: currentEvent.hints?.intent || records.find((record) => record.hints?.intent)?.hints?.intent || null,
    stop_hook_active: Boolean(input.stop_hook_active),
  };
}

function appendEventAndMaybeSnapshot(input) {
  const dir = continuityDir();
  const event = buildEvent(input);
  const telemetryEvents = [
    buildContextSavingsEvent(input, {
      source: "context-capture",
      eventType: "capture.event",
      decision: "recorded",
      reason: event.hook_event,
      delivered: false,
      inputBytes: byteLength(event.prompt_summary || event.assistant_summary || ""),
      indexedBytes: jsonLineBytes(event),
      metadata: {
        eventCount: 1,
        hookEvent: event.hook_event,
        hasPromptSummary: Boolean(event.prompt_summary),
        hasAssistantSummary: Boolean(event.assistant_summary),
      },
    }),
  ];
  appendJsonLine(path.join(dir, "events.jsonl"), event);
  appendJsonLine(path.join(dir, "sessions", `${safeName(event.provider)}-${safeName(event.session_id)}.jsonl`), event);

  if (event.hook_event === "PreCompact" || event.hook_event === "Stop") {
    const snapshot = buildSnapshot(input, event);
    appendJsonLine(path.join(dir, "snapshots.jsonl"), snapshot);
    appendJsonLine(path.join(dir, "sessions", `${safeName(event.provider)}-${safeName(event.session_id)}.snapshots.jsonl`), snapshot);
    telemetryEvents.push(buildContextSavingsEvent(input, {
      source: "context-capture",
      eventType: "capture.snapshot",
      decision: "recorded",
      reason: event.hook_event,
      delivered: false,
      inputBytes: byteLength(JSON.stringify(snapshot.prompt_summaries || [])),
      indexedBytes: jsonLineBytes(snapshot),
      metadata: {
        eventCount: 1,
        hookEvent: event.hook_event,
        promptSummaryCount: snapshot.prompt_summaries.length,
        decisionCount: snapshot.decisions.length,
        taskCount: snapshot.tasks.length,
      },
    }));
  }

  postContextSavingsEvents(telemetryEvents, "context-capture");
  return event;
}

function buildDirective(provider) {
  const records = recentRecords().filter((record) =>
    record.hook_event !== "SessionStart" && (!record.provider || record.provider === provider),
  );
  if (records.length === 0) return null;

  const lastPrompt = records.find((record) => record.prompt_summary)?.prompt_summary ||
    records.find((record) => Array.isArray(record.prompt_summaries) && record.prompt_summaries[0])?.prompt_summaries[0];
  const tasks = unique(records.flatMap((record) => record.tasks || record.hints?.tasks || []), 3);
  const decisions = unique(records.flatMap((record) => record.decisions || record.hints?.decisions || []), 3);
  const cwd = records.find((record) => record.cwd)?.cwd;

  const lines = [
    "<quill_continuity>",
    "Recent Quill continuity exists. Use it when resuming work; do not ask the user to repeat recent context.",
  ];
  if (cwd) lines.push(`cwd: ${compactText(cwd, 120)}`);
  if (lastPrompt) lines.push(`last_prompt: ${compactText(lastPrompt, 220)}`);
  if (tasks.length > 0) lines.push(`task_hints: ${tasks.join(" | ")}`);
  if (decisions.length > 0) lines.push(`decision_hints: ${decisions.join(" | ")}`);
  lines.push(`context_tools: ${CONTEXT_TOOLS.join(", ")}`);
  lines.push(provider === "codex"
    ? "Prefer Quill MCP tools over raw ~/.codex/sessions reads when looking up prior Codex work."
    : "Prefer Quill MCP tools over raw ~/.claude/projects reads when looking up prior Claude work.");
  lines.push("</quill_continuity>");
  return lines.join("\n");
}

function outputSessionStartDirective(input) {
  const provider = inferProvider(input);
  const directive = buildDirective(provider);
  if (!directive) return;
  postContextSavingsEvents([
    buildContextSavingsEvent(input, {
      source: "context-capture",
      provider,
      eventType: "capture.guidance",
      decision: "session-start-directive",
      reason: "recent-continuity",
      delivered: true,
      returnedBytes: byteLength(directive),
      tokensSavedEst: 0,
      tokensPreservedEst: 0,
      metadata: {
        eventCount: 1,
        hookEvent: "SessionStart",
      },
    }),
  ], "context-capture");

  process.stdout.write(`${JSON.stringify({
    hookSpecificOutput: {
      hookEventName: "SessionStart",
      additionalContext: directive,
    },
  })}\n`);
}

function handleInput(input) {
  const hook = eventName(input);
  if (!["SessionStart", "UserPromptSubmit", "PreCompact", "Stop"].includes(hook)) return;
  if (hook === "Stop" && input.stop_hook_active) return;

  if (hook === "SessionStart") {
    outputSessionStartDirective(input);
  }

  appendEventAndMaybeSnapshot(input);
}

function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    handleInput(JSON.parse(raw || "{}"));
  } catch (err) {
    if (process.env.QUILL_DEBUG) console.error("context-capture: error:", err.message);
  }
}

if (require.main === module) {
  main();
}

module.exports = { handleInput };
