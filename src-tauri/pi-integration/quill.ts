// quill-managed:pi
// quill-managed-pi-payload: 2
// Quill-managed Pi integration, payload/stamp 2.
// Disable Pi in Quill to remove this file.

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { hostname, homedir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";

// Keep equal to LOCAL_TIMEOUT_MS in ../codex-integration/scripts/lib.cjs.
const LOCAL_TIMEOUT_MS = 1500;
const CONTEXT_PORT = "19877";
const FEATURES = { context_preservation: true, activity_tracking: true, context_telemetry: true };
const TAINTED_MAX_PATHS = 256;
const READER_COMMAND_PATTERN =
  /\b(cat|bat|head|tail|less|more|view|od|xxd|strings|hexdump|sed|awk|grep|rg|ack|jq|yq|xq|xmllint)\b/i;
const BINARY_URL_EXT_RE =
  /\.(tar\.gz|tgz|tar\.bz2|tar\.xz|tar|zip|gz|bz2|xz|7z|rar|pdf|png|jpg|jpeg|gif|svg|webp|ico|woff2?|ttf|eot|mp3|mp4|mov|webm|wasm|exe|dmg|deb|rpm|whl)(\?|$)/i;

const objectSchema = (properties, required = []) => ({
  type: "object",
  properties,
  required,
  additionalProperties: false,
});
const string = (description) => ({ type: "string", description });
const integer = (description, minimum, maximum, defaultValue) => ({
  type: "integer",
  description,
  minimum,
  maximum,
  default: defaultValue,
});
const boolean = (description, defaultValue) => ({
  type: "boolean",
  description,
  default: defaultValue,
});

const TOOLS = [
  {
    name: "quill_search_history",
    label: "Search Quill history",
    description: "Search indexed Quill session history.",
    kind: "history",
    parameters: objectSchema(
      {
        query: string("Full-text search query."),
        project: string("Optional project working directory."),
        host: string("Optional hostname."),
        role: string("Optional user or assistant role."),
        git_branch: string("Optional git branch."),
        date_from: string("Optional start date in YYYY-MM-DD form."),
        date_to: string("Optional end date in YYYY-MM-DD form."),
        limit: integer("Maximum result count.", 1, 50, 10),
      },
      ["query"],
    ),
  },
  {
    name: "quill_index_context",
    label: "Index Quill context",
    description: "Index text or a file in Quill's local context store.",
    endpoint: "index",
    withCwd: true,
    parameters: objectSchema({
      content: string("Raw text to index; use content or file_path."),
      file_path: string("File path to index; use content or file_path."),
      cwd: string("Working directory for file resolution."),
      source: string("Source label."),
      content_type: {
        type: "string",
        enum: ["auto", "text", "markdown", "json", "code"],
        default: "auto",
      },
      max_bytes: integer("Maximum input bytes.", 1024, 5 * 1024 * 1024, 5 * 1024 * 1024),
    }),
  },
  {
    name: "quill_fetch_and_index",
    label: "Fetch and index Quill context",
    description: "Fetch a public HTTP(S) URL and index its bounded content.",
    endpoint: "fetch",
    parameters: objectSchema(
      {
        url: string("HTTP(S) URL to fetch."),
        source: string("Optional source label."),
        force: boolean("Bypass the 24-hour cache.", false),
        max_bytes: integer("Maximum response bytes.", 1024, 2 * 1024 * 1024, 2 * 1024 * 1024),
      },
      ["url"],
    ),
  },
  {
    name: "quill_execute",
    label: "Execute with Quill context",
    description: "Run a bounded local command and index large output.",
    endpoint: "execute",
    withCwd: true,
    parameters: objectSchema(
      {
        command: string("Shell command to execute."),
        cwd: string("Working directory."),
        timeout_ms: integer("Execution timeout in milliseconds.", 100, 120000, 30000),
        max_output_bytes: integer("Maximum stdout and stderr bytes.", 1024, 512 * 1024, 512 * 1024),
        index_output: boolean("Index large or truncated output.", true),
      },
      ["command"],
    ),
  },
  {
    name: "quill_search_context",
    label: "Search Quill context",
    description: "Search indexed working-context chunks and return bounded refs.",
    endpoint: "search",
    parameters: objectSchema(
      {
        query: string("Working-context search query."),
        source: string("Optional source label or source:N ref."),
        limit: integer("Maximum result count.", 1, 20, 5),
      },
      ["query"],
    ),
  },
  {
    name: "quill_get_context_source",
    label: "Get Quill context source",
    description: "Retrieve bounded source metadata or chunk content by ref.",
    endpoint: "source",
    parameters: objectSchema({
      source_ref: string("Source ref or numeric source id."),
      chunk_ref: string("Chunk ref or numeric chunk id."),
      source: string("Source label substring."),
      include_content: boolean("Return bounded chunk content.", false),
      limit: integer("Maximum chunks to list.", 1, 100, 20),
    }),
  },
  {
    name: "quill_context_stats",
    label: "Quill context stats",
    description: "Return compact local context-store statistics.",
    endpoint: "stats",
    parameters: objectSchema({}),
  },
  {
    name: "quill_purge_context",
    label: "Purge Quill context",
    description: "Purge one source or all local working-context data.",
    endpoint: "purge",
    parameters: objectSchema({
      confirm: boolean("Must be true to purge context data.", false),
      source_ref: string("Optional source:N ref or id to purge."),
    }),
  },
];

const EVENT_MAP = {
  session_start: "SessionStart",
  input: "UserPromptSubmit",
  tool_call: "PreToolUse",
  tool_result: "PostToolUse",
  turn_end: "Stop",
  session_shutdown: "SessionEnd",
  session_before_compact: "PreCompact",
  session_compact: "PostCompact",
};

function localBase(value) {
  const url = new URL(value);
  if (
    url.protocol !== "http:" ||
    url.username ||
    url.password ||
    !["localhost", "127.0.0.1", "[::1]"].includes(url.hostname.toLowerCase())
  ) {
    throw new Error("Quill URL must use loopback HTTP");
  }
  return url;
}

function loadConfig() {
  const root = process.env.HOME || process.env.USERPROFILE || homedir();
  const config = JSON.parse(readFileSync(join(root, ".config", "quill", "config.json"), "utf8"));
  if (typeof config.url !== "string" || typeof config.secret !== "string" || !config.secret) {
    throw new Error("Invalid Quill config");
  }
  const main = localBase(config.url);
  const context = config.context_url ? localBase(config.context_url) : new URL(main);
  if (!config.context_url) context.port = CONTEXT_PORT;
  return {
    main: main.origin,
    context: context.origin,
    secret: config.secret,
    home: root,
    markerRoot: join(root, ".config", "quill", "context", "markers"),
    hostname:
      (typeof config.hostname === "string" && config.hostname.trim()) ||
      hostname().split(".")[0] ||
      "local",
  };
}

function headers(config) {
  return {
    "Content-Type": "application/json",
    Authorization: `Bearer ${config.secret}`,
  };
}

async function fetchJson(config, url, options) {
  const response = await fetch(url, {
    ...options,
    headers: headers(config),
    signal: AbortSignal.timeout(LOCAL_TIMEOUT_MS),
  });
  if (!response.ok) throw new Error(`Quill returned ${response.status}`);
  return response.json();
}

function success(data) {
  return {
    content: [{ type: "text", text: JSON.stringify(data) }],
    details: { ok: true, data },
  };
}

function unavailable() {
  return {
    content: [{ type: "text", text: "Quill is unavailable." }],
    details: {
      ok: false,
      error: { type: "quill_unavailable", message: "Quill is unavailable." },
    },
    isError: true,
  };
}

function historyUrl(config, params) {
  const url = new URL("/api/v1/sessions/search", config.main);
  const names = {
    query: "q",
    limit: "page_size",
    project: "project",
    host: "host",
    role: "role",
    git_branch: "git_branch",
    date_from: "date_from",
    date_to: "date_to",
  };
  for (const [key, name] of Object.entries(names)) {
    if (params[key] !== undefined && params[key] !== null) {
      url.searchParams.set(name, String(params[key]));
    }
  }
  return url;
}

function stripHeredocs(command) {
  return command.replace(/<<-?\s*["']?([A-Za-z0-9_]+)["']?[\s\S]*?\n\s*\1/g, "");
}

function stripQuotedContent(command) {
  return stripHeredocs(command)
    .replace(/'[^']*'/g, "''")
    .replace(/"[^"]*"/g, '""');
}

function unquoteCommand(command) {
  return stripHeredocs(command)
    .replace(/'([^']*)'/g, "$1")
    .replace(/"([^"]*)"/g, "$1");
}

function hasRawNetworkDump(command) {
  const stripped = stripQuotedContent(command);
  if (!/(^|\s|&&|\|\||;)(curl|wget)\s/i.test(stripped)) return false;
  return stripped.split(/\s*(?:&&|\|\||;)\s*/).some((segment) => {
    const value = segment.trim();
    if (!/(^|\s)(curl|wget)\s/i.test(value) || /\s(-I|--head)(\s|$)/.test(value)) return false;
    const curl = /\bcurl\b/i.test(value);
    const fileOutput = curl
      ? /\s(-o|--output)\s+\S+/.test(value) || /\s(-O|--remote-name)(\s|$)/.test(value) || /\s>>?\s*\S+/.test(value)
      : /\s(-O|--output-document)\s+\S+/.test(value) || /\s>>?\s*\S+/.test(value);
    const quiet = curl
      ? /(^|\s)-[A-Za-z]*s[A-Za-z]*(\s|$)/.test(value) || /\s--silent(\s|$)/.test(value)
      : /(^|\s)-[A-Za-z]*q[A-Za-z]*(\s|$)/.test(value) || /\s--quiet(\s|$)/.test(value);
    const verbose = /\s(-v|--verbose|--trace|--trace-ascii|-D\s+-)(\s|$)/.test(value);
    const stdout = /\s(-o|--output|-O|--output-document)\s+(-|\/dev\/stdout)(\s|$)/.test(value);
    return !fileOutput || !quiet || verbose || stdout;
  });
}

function isInlineNetworkFetch(command) {
  const visible = stripHeredocs(command);
  return /fetch\s*\(\s*["']https?:\/\//i.test(visible) ||
    /requests\.(get|post|put|patch)\s*\(/i.test(visible) ||
    /http\.(get|request)\s*\(/i.test(visible);
}

function splitTopLevel(command) {
  const segments = [];
  let current = "";
  let quote = null;
  for (let index = 0; index < command.length; index += 1) {
    const character = command[index];
    if (quote) {
      current += character;
      if (character === quote) quote = null;
      continue;
    }
    if (character === "'" || character === '"') {
      quote = character;
      current += character;
      continue;
    }
    if (
      (character === "&" && command[index + 1] === "&") ||
      (character === "|" && command[index + 1] === "|")
    ) {
      segments.push(current);
      current = "";
      index += 1;
      continue;
    }
    if (character === ";") {
      segments.push(current);
      current = "";
      continue;
    }
    current += character;
  }
  segments.push(current);
  return segments;
}

function unquoteToken(token) {
  if (typeof token !== "string" || token.length < 2) return token;
  const first = token[0];
  const last = token[token.length - 1];
  return (first === "'" && last === "'") || (first === '"' && last === '"')
    ? token.slice(1, -1)
    : token;
}

function extractFetchOutputPaths(command) {
  const outputTarget = "('[^']*'|\"[^\"]*\"|[^\\s]+)";
  const paths = [];
  for (const segment of splitTopLevel(stripHeredocs(command))) {
    const bare = stripQuotedContent(segment);
    if (!/(?:^|\s)(?:curl|wget)(?:\s|$)/i.test(bare) || /(?:^|\s)(?:-I|--head)(?:\s|$)/.test(bare)) {
      continue;
    }
    const wget = /(?:^|\s)wget(?:\s|$)/i.test(bare);
    const flags = wget ? "-[oO]" : "-o";
    const flagPattern = new RegExp(
      `(?:^|\\s)(?:(?:--output-document|--output)(?:\\s+|=)|${flags}\\s+)${outputTarget}`,
      "g",
    );
    for (const match of segment.matchAll(flagPattern)) {
      const path = unquoteToken(match[1]);
      if (path && !["-", "/dev/stdout", "/dev/null"].includes(path)) paths.push(path);
    }
    const redirectPattern = new RegExp(`(?:^|\\s)>>?\\s*${outputTarget}`, "g");
    for (const match of segment.matchAll(redirectPattern)) {
      const path = unquoteToken(match[1]);
      if (path && !["/dev/stdout", "/dev/null"].includes(path)) paths.push(path);
    }
  }
  return paths;
}

function isDegenerateTaint(path) {
  if (typeof path !== "string" || !path.trim()) return true;
  return /^["']+$/.test(path.trim()) || /^["']+$/.test(basename(path.trim()));
}

function resolveLiteralPath(config, cwd, path) {
  if (!path) return path;
  let resolved = path.startsWith("~") ? join(config.home, path.slice(1)) : path;
  if (!isAbsolute(resolved)) resolved = resolve(cwd || process.cwd(), resolved);
  return resolved;
}

function taintedStatePath(config, sessionId) {
  const safeSession = String(sessionId || "unknown").replace(/[^a-zA-Z0-9._-]+/g, "_").slice(0, 120);
  return join(config.markerRoot, `pi-${safeSession}`, "tainted.json");
}

function loadTainted(config, sessionId) {
  try {
    const state = JSON.parse(readFileSync(taintedStatePath(config, sessionId), "utf8"));
    return new Set((Array.isArray(state.paths) ? state.paths : []).filter((path) => !isDegenerateTaint(path)));
  } catch {
    return new Set();
  }
}

function saveTainted(config, sessionId, paths) {
  try {
    const statePath = taintedStatePath(config, sessionId);
    const bounded = [...paths].slice(-TAINTED_MAX_PATHS);
    mkdirSync(dirname(statePath), { recursive: true });
    writeFileSync(statePath, JSON.stringify({ paths: bounded }), "utf8");
  } catch {
    // Taint persistence is best effort; routing must keep working.
  }
}

function recordTainted(config, sessionId, cwd, paths) {
  if (!paths.length) return;
  const tainted = loadTainted(config, sessionId);
  for (const path of paths) {
    if (isDegenerateTaint(path)) continue;
    tainted.add(path);
    const resolved = resolveLiteralPath(config, cwd, path);
    if (resolved && resolved !== path && !isDegenerateTaint(resolved)) tainted.add(resolved);
  }
  saveTainted(config, sessionId, tainted);
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function commandReadsTaintedPath(command, tainted) {
  if (!command || !tainted.size) return null;
  const stripped = stripQuotedContent(command);
  if (!READER_COMMAND_PATTERN.test(stripped)) return null;
  const unquoted = unquoteCommand(command);
  for (const path of tainted) {
    if (isDegenerateTaint(path)) continue;
    const pattern = new RegExp(`(?:^|[\\s=])${escapeRegExp(path)}(?:[\\s)>;|&]|$)`);
    if (pattern.test(stripped) || pattern.test(unquoted)) return path;
  }
  return null;
}

function readTargetsTaintedPath(config, cwd, path, tainted) {
  if (!path || !tainted.size) return null;
  if (tainted.has(path)) return path;
  const resolved = resolveLiteralPath(config, cwd, path);
  return resolved && tainted.has(resolved) ? resolved : null;
}

function extractFetchUrls(command) {
  const urls = [];
  const seen = new Set();
  const pattern = /https?:\/\/[^\s<>|`]+/gi;
  let match;
  while ((match = pattern.exec(stripHeredocs(command || ""))) !== null) {
    let url = match[0];
    const quote = url.search(/["']/);
    if (quote >= 0) url = url.slice(0, quote);
    url = url.replace(/[\r\n\t]+/g, "").replace(/[.,;:!?\]]+$/g, "");
    while (url.endsWith(")")) {
      const opens = (url.match(/\(/g) || []).length;
      const closes = (url.match(/\)/g) || []).length;
      if (closes <= opens) break;
      url = url.slice(0, -1);
    }
    if (url && !seen.has(url)) {
      seen.add(url);
      urls.push(url);
      if (urls.length === 2) break;
    }
  }
  return urls;
}

function looksLikeApiJson(url) {
  return !BINARY_URL_EXT_RE.test(url) && (
    /^https?:\/\/api\./i.test(url) || /[?&]format=json|\.json(\?|$)|\/api\//i.test(url)
  );
}

function fetchDenyReason(command, explicitUrl) {
  const urls = explicitUrl ? [explicitUrl] : extractFetchUrls(command);
  const lines = ["Quill context routing blocked a raw network fetch."];
  if (urls.length) {
    lines.push("", "Run this instead:");
    for (const url of urls) {
      lines.push(`  quill_fetch_and_index(url=${JSON.stringify(url)})`);
      if (looksLikeApiJson(url)) {
        lines.push(`  quill_execute(command=${JSON.stringify(`curl -sS ${url} | jq .`)})`);
      }
    }
    lines.push("", "Then use quill_search_context to retrieve focused chunks.");
  } else {
    lines.push("Use quill_execute for a bounded curl and jq workflow, or quill_fetch_and_index for pages.");
  }
  lines.push(
    "",
    "Do not bypass this by fetching to a file and reading it back with cat, jq, grep, sed, awk, or read.",
    "Use curl or wget file output only for binary artifacts you will run or install.",
  );
  return lines.join("\n");
}

function taintedReadDenyReason(tool, path) {
  return [
    `Quill context routing blocked ${tool} on ${path} because that path was written by an earlier curl/wget in this session.`,
    "Reading freshly fetched network content into the transcript defeats context routing.",
    "Use quill_search_context if the response was indexed, or quill_execute to re-fetch with bounded output.",
    "If the path now holds unrelated scratch data, use another filename and try again.",
  ].join("\n");
}

function postRoutingTelemetry(config, event, ctx, reason, route) {
  if (!FEATURES.context_telemetry) return;
  try {
    const timestamp = new Date().toISOString();
    const sessionId = ctx.sessionManager.getSessionId();
    const body = {
      eventId: "",
      schemaVersion: 1,
      provider: "pi",
      sessionId,
      hostname: config.hostname,
      cwd: ctx.cwd || null,
      timestamp,
      eventType: "router.denial",
      source: "context-router",
      decision: "deny",
      category: "routing",
      reason,
      delivered: true,
      indexedBytes: null,
      returnedBytes: Buffer.byteLength(reason, "utf8"),
      inputBytes: Buffer.byteLength(JSON.stringify(event.input || {}), "utf8"),
      tokensIndexedEst: 0,
      tokensReturnedEst: 0,
      tokensSavedEst: 0,
      tokensPreservedEst: 0,
      estimateMethod: "none",
      estimateConfidence: 0,
      sourceRef: null,
      metadata: { eventCount: 1, toolName: event.toolName, route },
    };
    body.eventId = `ctx_${createHash("sha256").update(JSON.stringify(body)).digest("hex").slice(0, 32)}`;
    void fetch(`${config.main}/api/v1/context-savings/events`, {
      method: "POST",
      headers: headers(config),
      body: JSON.stringify({ events: [body] }),
      signal: AbortSignal.timeout(LOCAL_TIMEOUT_MS),
    }).then(
      (response) => response.body?.cancel().catch(() => {}),
      () => {},
    );
  } catch {
    // Routing telemetry never changes the routing decision.
  }
}

function deny(config, event, ctx, reason, route) {
  postRoutingTelemetry(config, event, ctx, reason, route);
  return { block: true, reason };
}

function routeToolCall(config, event, ctx) {
  if (!event || typeof event.toolName !== "string") return undefined;
  const sessionId = ctx.sessionManager.getSessionId();
  const input = event.input || {};
  if (["fetch", "web_fetch", "webfetch"].includes(event.toolName)) {
    const url = typeof input.url === "string" ? input.url : null;
    return deny(config, event, ctx, fetchDenyReason("", url), "webfetch");
  }
  if (event.toolName === "bash") {
    const command = typeof input.command === "string" ? input.command : "";
    if (!command) return undefined;
    if (hasRawNetworkDump(command) || isInlineNetworkFetch(command)) {
      return deny(config, event, ctx, fetchDenyReason(command), "raw-network-fetch");
    }
    const tainted = loadTainted(config, sessionId);
    const hit = commandReadsTaintedPath(command, tainted);
    if (hit) return deny(config, event, ctx, taintedReadDenyReason("bash", hit), "tainted-read-bash");
    recordTainted(config, sessionId, ctx.cwd, extractFetchOutputPaths(command));
    return undefined;
  }
  if (event.toolName === "read") {
    const path = typeof input.path === "string" ? input.path : "";
    const hit = readTargetsTaintedPath(config, ctx.cwd, path, loadTainted(config, sessionId));
    if (hit) return deny(config, event, ctx, taintedReadDenyReason("read", hit), "tainted-read");
  }
  return undefined;
}

function postTelemetry(config, event, ctx, hookEvent) {
  const payload = {
    provider: "pi",
    session_id: ctx.sessionManager.getSessionId(),
    hostname: config.hostname,
    hook_event: hookEvent,
    tool_name: event.toolName || null,
    cwd: ctx.cwd || null,
    ts: new Date().toISOString(),
    hook_matcher: null,
    agent_id: null,
  };
  void fetch(`${config.main}/api/v1/hooks/observed`, {
    method: "POST",
    headers: headers(config),
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(LOCAL_TIMEOUT_MS),
  }).then(
    (response) => response.body?.cancel().catch(() => {}),
    () => {},
  );
}

// @lat: [[infrastructure#Infrastructure#Pi Integration Deployment#Extension Tools and Telemetry]]
export default function quill(pi) {
  let config;
  try {
    config = loadConfig();
  } catch {
    return;
  }

  if (FEATURES.context_preservation) {
    for (const tool of TOOLS) {
      try {
        pi.registerTool({
          name: tool.name,
          label: tool.label,
          description: tool.description,
          parameters: tool.parameters,
          async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
            try {
              if (tool.kind === "history") {
                return success(await fetchJson(config, historyUrl(config, params), { method: "GET" }));
              }
              const payload = { ...params };
              if (tool.withCwd && !payload.cwd) payload.cwd = ctx.cwd;
              const data = await fetchJson(config, `${config.context}/api/v1/context/${tool.endpoint}`, {
                method: "POST",
                body: JSON.stringify(payload),
              });
              return success(data);
            } catch {
              return unavailable();
            }
          },
        });
      } catch {
        // A duplicate or incompatible registration must not stop Pi loading.
      }
    }
    try {
      pi.on("tool_call", (event, ctx) => {
        try {
          return routeToolCall(config, event, ctx);
        } catch {
          return undefined;
        }
      });
    } catch {
      // Event API drift self-disables context routing.
    }
  }

  if (FEATURES.activity_tracking) {
    for (const [eventName, hookEvent] of Object.entries(EVENT_MAP)) {
      try {
        pi.on(eventName, (event, ctx) => {
          try {
            postTelemetry(config, event, ctx, hookEvent);
          } catch {
            // Telemetry never alters Pi lifecycle behavior.
          }
        });
      } catch {
        // Event API drift self-disables this one observation.
      }
    }
  }
}
