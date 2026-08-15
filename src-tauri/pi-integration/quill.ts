// quill-managed:pi
// quill-managed-pi-payload: 2
// Quill-managed Pi integration, payload/stamp 2.
// Disable Pi in Quill to remove this file.

import { createHash, randomUUID } from "node:crypto";
import {
  appendFileSync,
  chmodSync,
  closeSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { hostname, homedir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";

// Keep equal to LOCAL_TIMEOUT_MS in ../codex-integration/scripts/lib.cjs.
const LOCAL_TIMEOUT_MS = 1500;
const CONTEXT_PORT = "19877";
const FEATURES = { context_preservation: true, activity_tracking: true, context_telemetry: true };
const PROTOCOL_VERSION = 1;
export const EXTENSION_VERSION = "0.1.0";
const MIN_QUILL_VERSION = "0.9.0";
const SPOOL_FILE_MAX_BYTES = 1024 * 1024;
const SPOOL_DIR_MAX_BYTES = 16 * SPOOL_FILE_MAX_BYTES;
const SPOOL_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;
const LOG_MAX_BYTES = 256 * 1024;
const TAINTED_MAX_PATHS = 256;
const REPORTER_CLAIMS = Symbol.for("quill.pi.reporter.claims");
let lastNoticeRoot;
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
  agent_start: "SubagentStart",
  agent_settled: "SubagentStop",
  tool_execution_start: "PreToolUse",
  tool_execution_end: "PostToolUse",
  tool_call: "PreToolUse",
  tool_result: "PostToolUse",
  turn_end: "Stop",
  session_shutdown: "SessionEnd",
  session_before_compact: "PreCompact",
  session_compact: "PostCompact",
};
const TRACKING_EVENTS = new Set([
  "session_start",
  "session_shutdown",
  "agent_start",
  "agent_settled",
  "turn_start",
  "turn_end",
  "message_end",
  "tool_execution_start",
  "tool_execution_end",
  "model_select",
  "input",
]);

export class QuillExtensionError extends Error {
  constructor(message, options = {}) {
    super(message, options);
    this.name = new.target.name;
  }
}

export class ConfigError extends QuillExtensionError {}
export class TransportError extends QuillExtensionError {}
export class ProtocolMismatchError extends QuillExtensionError {}
export class RegistrationError extends QuillExtensionError {}
export class SpoolError extends QuillExtensionError {}

function errorMessage(error) {
  return error instanceof Error ? `${error.name}: ${error.message}` : `Error: ${String(error)}`;
}

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
  let config;
  try {
    config = JSON.parse(readFileSync(join(root, ".config", "quill", "config.json"), "utf8"));
  } catch (error) {
    throw new ConfigError("Quill config is missing or malformed", { cause: error });
  }
  if (typeof config.url !== "string" || typeof config.secret !== "string" || !config.secret) {
    throw new ConfigError("Invalid Quill config");
  }
  let main;
  let context;
  try {
    main = localBase(config.url);
    context = config.context_url ? localBase(config.context_url) : new URL(main);
  } catch (error) {
    throw new ConfigError("Invalid Quill config URL", { cause: error });
  }
  if (!config.context_url) context.port = CONTEXT_PORT;
  const quillRoot = join(root, ".config", "quill");
  return {
    main: main.origin,
    context: context.origin,
    secret: config.secret,
    home: root,
    quillRoot,
    spoolRoot: join(quillRoot, "pi-spool"),
    logPath: join(quillRoot, "pi-extension.log"),
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

function claimReporter(config) {
  const claims = globalThis[REPORTER_CLAIMS] || new Set();
  globalThis[REPORTER_CLAIMS] = claims;
  if (claims.has(config.quillRoot)) return false;
  claims.add(config.quillRoot);
  return true;
}

function releaseReporter(config) {
  globalThis[REPORTER_CLAIMS]?.delete(config.quillRoot);
}

async function fetchJson(config, url, options) {
  const response = await fetch(url, {
    ...options,
    headers: headers(config),
    signal: AbortSignal.timeout(LOCAL_TIMEOUT_MS),
  });
  if (!response.ok) throw new TransportError(`Quill returned ${response.status}`);
  return response.json();
}

function safeSessionId(value) {
  return String(value || "unknown").replace(/[^a-zA-Z0-9._-]+/g, "_").slice(0, 120);
}

function writeLog(config, error) {
  try {
    const line = `${new Date().toISOString()} ${errorMessage(error)}\n`;
    mkdirSync(config.quillRoot, { recursive: true, mode: 0o700 });
    let size = 0;
    try {
      size = statSync(config.logPath).size;
    } catch (statError) {
      if (statError?.code !== "ENOENT") throw statError;
    }
    if (size + Buffer.byteLength(line) > LOG_MAX_BYTES) {
      writeFileSync(config.logPath, line, { mode: 0o600 });
    } else {
      appendFileSync(config.logPath, line, { mode: 0o600 });
    }
    chmodSync(config.logPath, 0o600);
  } catch (logError) {
    console.error("Quill Pi extension log failure", errorMessage(logError));
  }
}

function spoolFile(config, sessionId) {
  return join(config.spoolRoot, `${safeSessionId(sessionId)}.${process.pid}.jsonl`);
}

function pruneSpool(config, incomingBytes, currentPath) {
  const now = Date.now();
  const files = readdirSync(config.spoolRoot)
    .filter((name) => name.endsWith(".jsonl"))
    .map((name) => {
      const path = join(config.spoolRoot, name);
      return { path, ...statSync(path) };
    })
    .sort((left, right) => left.mtimeMs - right.mtimeMs);
  let total = files.reduce((sum, file) => sum + file.size, 0);
  for (const file of files) {
    if (file.path !== currentPath && (now - file.mtimeMs > SPOOL_MAX_AGE_MS || total + incomingBytes > SPOOL_DIR_MAX_BYTES)) {
      unlinkSync(file.path);
      total -= file.size;
    }
  }
  return total;
}

function appendSpool(config, endpoint, payload) {
  const sessionId = payload.session_id || payload.events?.[0]?.session_id;
  const line = `${JSON.stringify({ endpoint, payload })}\n`;
  const bytes = Buffer.byteLength(line);
  if (bytes > SPOOL_FILE_MAX_BYTES) throw new SpoolError("Spool record exceeds file cap");
  try {
    mkdirSync(config.spoolRoot, { recursive: true, mode: 0o700 });
    chmodSync(config.spoolRoot, 0o700);
    const path = spoolFile(config, sessionId);
    let fileBytes = 0;
    try {
      fileBytes = statSync(path).size;
    } catch (statError) {
      if (statError?.code !== "ENOENT") throw statError;
    }
    pruneSpool(config, bytes, path);
    if (fileBytes + bytes > SPOOL_FILE_MAX_BYTES) {
      throw new SpoolError("Spool file cap reached; newest event dropped");
    }
    appendFileSync(path, line, { mode: 0o600 });
    chmodSync(path, 0o600);
  } catch (error) {
    if (error instanceof SpoolError) throw error;
    throw new SpoolError("Failed to append Pi tracking spool", { cause: error });
  }
}

async function responseError(response) {
  let body;
  try {
    body = await response.json();
  } catch (error) {
    body = null;
  }
  if (response.status === 400 && body?.error === "protocol_mismatch") {
    return new ProtocolMismatchError(body.message || "Pi tracking protocol mismatch");
  }
  return new TransportError(body?.message || `Quill returned ${response.status}`);
}

async function postPayload(config, endpoint, payload, retryAuth = true) {
  let response;
  try {
    response = await fetch(`${config.main}${endpoint}`, {
      method: "POST",
      headers: headers(config),
      body: JSON.stringify(payload),
      signal: AbortSignal.timeout(LOCAL_TIMEOUT_MS),
    });
  } catch (error) {
    throw new TransportError("Quill request failed", { cause: error });
  }
  if (response.status === 401 && retryAuth) {
    Object.assign(config, loadConfig());
    return postPayload(config, endpoint, payload, false);
  }
  if (!response.ok) throw await responseError(response);
  try {
    await response.body?.cancel();
  } catch (error) {
    writeLog(config, new TransportError("Failed to release Quill response", { cause: error }));
  }
}

function sendTracked(config, state, endpoint, payload) {
  let request;
  try {
    request = postPayload(config, endpoint, payload);
  } catch (error) {
    request = Promise.reject(error);
  }
  return request.then(
    () => true,
    (error) => {
      const failure = error instanceof QuillExtensionError
        ? error
        : new TransportError("Unexpected Pi tracking failure", { cause: error });
      state.lastError = failure.name;
      writeLog(config, failure);
      try {
        appendSpool(config, endpoint, payload);
      } catch (spoolError) {
        writeLog(config, spoolError);
      }
      return false;
    },
  );
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
  } catch (error) {
    return new Set();
  }
}

function saveTainted(config, sessionId, paths) {
  try {
    const statePath = taintedStatePath(config, sessionId);
    const bounded = [...paths].slice(-TAINTED_MAX_PATHS);
    mkdirSync(dirname(statePath), { recursive: true });
    writeFileSync(statePath, JSON.stringify({ paths: bounded }), "utf8");
  } catch (error) {
    writeLog(config, new QuillExtensionError("Taint persistence failed", { cause: error }));
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
      (response) => response.body?.cancel().catch((error) => writeLog(config, error)),
      (error) => writeLog(config, new TransportError("Routing telemetry failed", { cause: error })),
    );
  } catch (error) {
    writeLog(config, new TransportError("Routing telemetry failed", { cause: error }));
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
    (response) => response.body?.cancel().catch((error) => writeLog(config, error)),
    (error) => writeLog(config, new TransportError("Hook telemetry failed", { cause: error })),
  );
}

function isoTimestamp(value) {
  const date = value === undefined ? new Date() : new Date(value);
  return Number.isNaN(date.valueOf()) ? new Date().toISOString() : date.toISOString();
}

function stableId(prefix, ...parts) {
  return `${prefix}_${createHash("sha256").update(JSON.stringify(parts)).digest("hex").slice(0, 32)}`;
}

function sessionInfo(ctx) {
  const manager = ctx.sessionManager;
  const header = manager.getHeader?.();
  return {
    id: header?.id || manager.getSessionId(),
    file: manager.getSessionFile?.(),
    header,
    cwd: ctx.cwd || header?.cwd || null,
  };
}

function readSessionId(path) {
  let handle;
  try {
    handle = openSync(path, "r");
    const buffer = Buffer.allocUnsafe(64 * 1024);
    const length = readSync(handle, buffer, 0, buffer.length, 0);
    const firstLine = buffer.toString("utf8", 0, length).split("\n", 1)[0];
    const header = JSON.parse(firstLine);
    if (header?.type !== "session" || typeof header.id !== "string" || !header.id) {
      throw new ConfigError("Parent session header has no stable id");
    }
    return header.id;
  } catch (error) {
    if (error instanceof QuillExtensionError) throw error;
    throw new ConfigError("Cannot resolve parent session", { cause: error });
  } finally {
    if (handle !== undefined) closeSync(handle);
  }
}

function resolveStart(event, info) {
  const parentPath = info.header?.parentSession || (event.reason === "fork" ? event.previousSessionFile : undefined);
  const resolved = new Map();
  const resolveId = (path) => {
    if (!resolved.has(path)) resolved.set(path, readSessionId(path));
    return resolved.get(path);
  };
  let lineage = { kind: "root" };
  if (parentPath) {
    try {
      lineage = { kind: "linked", parent_session_id: resolveId(parentPath) };
    } catch (error) {
      lineage = { kind: "unresolved", reason: "parent_header_unavailable" };
    }
  }
  let previousSessionId;
  if (["new", "resume", "fork"].includes(event.reason) && event.previousSessionFile) {
    try {
      previousSessionId = resolveId(event.previousSessionFile);
    } catch (error) {
      previousSessionId = undefined;
    }
  }
  return { lineage, previousSessionId };
}

function trackEnvelope(state, events) {
  return {
    protocol: PROTOCOL_VERSION,
    extension_version: EXTENSION_VERSION,
    min_quill_version: MIN_QUILL_VERSION,
    ...(state.lastError ? { last_error: state.lastError } : {}),
    events,
  };
}

function trackEvent(config, state, info, type, fields = {}, identity = randomUUID()) {
  const timestamp = fields.timestamp || isoTimestamp();
  const event = {
    type,
    event_uuid: stableId("pi", info.id, type, identity),
    session_id: info.id,
    hostname: config.hostname,
    timestamp,
    ...fields,
  };
  return sendTracked(config, state, "/api/v1/pi/track", trackEnvelope(state, [event]));
}

function trackEvents(config, state, info, events) {
  return sendTracked(config, state, "/api/v1/pi/track", trackEnvelope(state, events));
}

function runtimeMessage(config, state, info, message) {
  const payload = {
    provider: "pi",
    host: config.hostname,
    session_id: info.id,
    project: info.cwd || "unknown",
    cwd: info.cwd,
    messages: [{
      uuid: message.uuid,
      type: message.type,
      timestamp: message.timestamp,
      content: "",
      role: message.role,
      tools_used: message.tools || [],
      files_modified: [],
      event_kinds: message.eventKinds,
    }],
  };
  return sendTracked(config, state, "/api/v1/sessions/messages", payload);
}

function notifySession(config, state, info, lineage) {
  if (!info.file) return Promise.resolve(true);
  const payload = {
    provider: "pi",
    session_id: info.id,
    jsonl_path: info.file,
    host: config.hostname,
    cwd: info.cwd,
    project: info.cwd,
    lineage,
  };
  return postPayload(config, "/api/v1/sessions/notify", payload).then(
    () => true,
    (error) => {
      state.lastError = error.name;
      writeLog(config, error);
      return false;
    },
  );
}

function defer(config, task) {
  queueMicrotask(() => {
    try {
      void task();
    } catch (error) {
      writeLog(config, new TransportError("Pi tracking handler failed", { cause: error }));
    }
  });
}

function registerHandler(pi, config, event, handler) {
  try {
    pi.on(event, handler);
  } catch (error) {
    writeLog(config, new RegistrationError(`Cannot register ${event}`, { cause: error }));
  }
}

function registerTracking(pi, config) {
  const state = { lastError: null, notify: null };
  const activity = (event, ctx, name) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      postTelemetry(config, event, ctx, EVENT_MAP[name]);
      return trackEvent(config, state, info, "activity", {}, `${name}:${randomUUID()}`);
    });
  };

  registerHandler(pi, config, "session_start", (event, ctx) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      const { lineage, previousSessionId } = resolveStart(event, info);
      state.notify = { info, lineage };
      const timestamp = isoTimestamp(info.header?.timestamp);
      postTelemetry(config, event, ctx, EVENT_MAP.session_start);
      const handshake = trackEvent(config, state, info, "session_start", {
        timestamp,
        cwd: info.cwd,
        ephemeral: !info.file,
        reason: event.reason,
        ...(previousSessionId ? { previous_session_id: previousSessionId } : {}),
        lineage,
      }, `${event.reason}:${timestamp}`);
      void notifySession(config, state, info, lineage);
      return handshake;
    });
  });
  registerHandler(pi, config, "session_shutdown", async (event, ctx) => {
    try {
      const info = sessionInfo(ctx);
      postTelemetry(config, event, ctx, EVENT_MAP.session_shutdown);
      await trackEvent(config, state, info, "session_end", { reason: event.reason }, `${event.reason}:${event.targetSessionFile || ""}`);
    } catch (error) {
      writeLog(config, new TransportError("Pi shutdown tracking failed", { cause: error }));
    } finally {
      releaseReporter(config);
    }
  });
  registerHandler(pi, config, "agent_start", (event, ctx) => activity(event, ctx, "agent_start"));
  registerHandler(pi, config, "agent_settled", (event, ctx) => activity(event, ctx, "agent_settled"));
  registerHandler(pi, config, "turn_start", (event, ctx) => activity(event, ctx, "turn_start"));
  registerHandler(pi, config, "input", (event, ctx) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      const timestamp = isoTimestamp();
      postTelemetry(config, event, ctx, EVENT_MAP.input);
      void trackEvent(config, state, info, "activity", { timestamp }, `input:${randomUUID()}`);
      return runtimeMessage(config, state, info, {
        uuid: stableId("pi_msg", info.id, "input", timestamp),
        type: "user",
        timestamp,
        role: "user",
        eventKinds: ["user_text"],
      });
    });
  });
  registerHandler(pi, config, "tool_execution_start", (event, ctx) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      const timestamp = isoTimestamp();
      postTelemetry(config, event, ctx, EVENT_MAP.tool_execution_start);
      void trackEvent(config, state, info, "activity", { timestamp }, `tool-start:${event.toolCallId}`);
      return runtimeMessage(config, state, info, {
        uuid: stableId("pi_msg", info.id, "tool-start", event.toolCallId),
        type: "assistant_tool_use",
        timestamp,
        role: "assistant",
        tools: [event.toolName],
        eventKinds: ["asst_tool_use"],
      });
    });
  });
  registerHandler(pi, config, "tool_execution_end", (event, ctx) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      const timestamp = isoTimestamp();
      postTelemetry(config, event, ctx, EVENT_MAP.tool_execution_end);
      void trackEvent(config, state, info, "activity", { timestamp }, `tool-end:${event.toolCallId}`);
      return runtimeMessage(config, state, info, {
        uuid: stableId("pi_msg", info.id, "tool-end", event.toolCallId),
        type: "tool_result",
        timestamp,
        role: "user",
        eventKinds: ["user_tool_result"],
      });
    });
  });
  registerHandler(pi, config, "model_select", (event, ctx) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      return trackEvent(config, state, info, "model", {
        model_provider: event.model.provider,
        model: event.model.id,
      }, `model:${event.model.provider}:${event.model.id}:${randomUUID()}`);
    });
  });
  registerHandler(pi, config, "message_end", (event, ctx) => {
    if (event.message?.role !== "assistant" || !event.message.usage) return;
    defer(config, () => {
      const info = sessionInfo(ctx);
      const message = event.message;
      const timestamp = isoTimestamp(message.timestamp);
      const cost = message.usage.cost || {};
      const identity = message.responseId || stableId("response", info.id, timestamp, message.provider, message.model);
      const common = {
        session_id: info.id,
        hostname: config.hostname,
        timestamp,
      };
      return trackEvents(config, state, info, [
        {
          ...common,
          type: "model",
          event_uuid: stableId("pi", info.id, "message-model", identity),
          model_provider: message.provider,
          model: message.model,
        },
        {
          ...common,
          type: "usage",
          event_uuid: stableId("pi", info.id, "usage", identity),
          model_provider: message.provider,
          model: message.model,
          input_tokens: Number(message.usage.input || 0),
          output_tokens: Number(message.usage.output || 0),
          cache_read_tokens: Number(message.usage.cacheRead || 0),
          cache_write_tokens: Number(message.usage.cacheWrite || 0),
          cost: {
            input: Number(cost.input || 0),
            output: Number(cost.output || 0),
            cache_read: Number(cost.cacheRead || 0),
            cache_write: Number(cost.cacheWrite || 0),
            total: Number(cost.total || 0),
          },
        },
      ]);
    });
  });
  registerHandler(pi, config, "turn_end", (event, ctx) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      const timestamp = isoTimestamp(event.message?.timestamp);
      postTelemetry(config, event, ctx, EVENT_MAP.turn_end);
      void trackEvent(config, state, info, "activity", { timestamp }, `turn:${event.turnIndex}`);
      if (state.notify) {
        void notifySession(config, state, state.notify.info, state.notify.lineage);
      }
      return runtimeMessage(config, state, info, {
        uuid: stableId("pi_msg", info.id, "turn", event.turnIndex, timestamp),
        type: "assistant",
        timestamp,
        role: "assistant",
        eventKinds: ["asst_text"],
      });
    });
  });
}

// @lat: [[infrastructure#Infrastructure#Pi Integration Deployment#Extension Tools and Telemetry]]
export default function quill(pi) {
  let config;
  try {
    config = loadConfig();
  } catch (error) {
    const root = process.env.HOME || process.env.USERPROFILE || homedir();
    if (lastNoticeRoot !== root) {
      lastNoticeRoot = root;
      console.warn("Quill Pi extension inactive: install or repair Quill config.");
    }
    return;
  }
  if (!claimReporter(config)) return;

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
            } catch (error) {
              return unavailable();
            }
          },
        });
      } catch (error) {
        writeLog(config, new RegistrationError(`Cannot register ${tool.name}`, { cause: error }));
      }
    }
    registerHandler(pi, config, "tool_call", (event, ctx) => {
      try {
        return routeToolCall(config, event, ctx);
      } catch (error) {
        writeLog(config, new QuillExtensionError("Context routing failed", { cause: error }));
        return undefined;
      }
    });
  }

  if (FEATURES.activity_tracking) {
    registerTracking(pi, config);
    for (const [eventName, hookEvent] of Object.entries(EVENT_MAP)) {
      if (TRACKING_EVENTS.has(eventName)) continue;
      registerHandler(pi, config, eventName, (event, ctx) => {
        try {
          postTelemetry(config, event, ctx, hookEvent);
        } catch (error) {
          writeLog(config, new TransportError("Hook telemetry failed", { cause: error }));
        }
      });
    }
  } else {
    registerHandler(pi, config, "session_shutdown", () => releaseReporter(config));
  }
}
