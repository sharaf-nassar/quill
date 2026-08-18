// quill-managed:pi
// quill-managed-pi-payload: 3
// Quill-managed Pi integration, payload/stamp 3.
// Disable Pi in Quill to remove this file.
// @ts-nocheck - untyped Node payload loaded by Pi, outside this repo's TS
// program (tsconfig covers src/ only and there is no @types/node).

import { createHash, randomUUID } from "node:crypto";
import {
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  writeFileSync,
} from "node:fs";
import { hostname, homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Keep equal to LOCAL_TIMEOUT_MS in ../codex-integration/scripts/lib.cjs.
const LOCAL_TIMEOUT_MS = 1500;
const CONTEXT_PORT = "19877";
const FEATURES = {
  context_preservation: true,
  activity_tracking: true,
  context_telemetry: true,
};
const PROTOCOL_VERSION = 1;
export const EXTENSION_VERSION = "0.2.0";
const MIN_QUILL_VERSION = "0.9.0";

export const PI_PROTOCOL_V2 = 2;
export const PI_PROTOCOL_V2_REPORTER_VERSION = EXTENSION_VERSION;
export const PI_PROTOCOL_V2_QUILL_BUILD = "0.0.0-injected-by-ci";
export const PI_PROTOCOL_V2_TRACKING_SCHEMA = 2;
export const PI_PROTOCOL_V2_CAPABILITIES = Object.freeze([
  "direct-lineage",
  "lifecycle-occurrence",
  "persisted-session-entry",
  "typed-outcomes",
  "candidate-broker",
]);
export const PI_PROTOCOL_V2_CAPABILITY_DIGEST = createHash("sha256")
  .update(PI_PROTOCOL_V2_CAPABILITIES.join("\n"))
  .digest("hex");
const TAINTED_MAX_PATHS = 256;
// Keep equal to sessions::COMPACT_SEARCH_MAX_BYTES.
const HISTORY_RESULT_MAX_BYTES = 32 * 1024;
const REPORTER_CLAIMS = Symbol.for("quill.pi.reporter.claims");
const REPORTER_BROKERS = Symbol.for("quill.pi.reporter.brokers.v1");
const REPORTER_NOTICES = Symbol.for("quill.pi.reporter.notices.v1");
const REPORTER_PROCESS = Symbol.for("quill.pi.reporter.process");
const REPORTER_CHANNEL_RANK = Object.freeze({
  managed: 0,
  npm: 1,
  project: 2,
  development: 3,
});
const EXTENSION_PATH = fileURLToPath(import.meta.url);
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
      max_bytes: integer(
        "Maximum input bytes.",
        1024,
        5 * 1024 * 1024,
        5 * 1024 * 1024,
      ),
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
        max_bytes: integer(
          "Maximum response bytes.",
          1024,
          2 * 1024 * 1024,
          2 * 1024 * 1024,
        ),
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
        timeout_ms: integer(
          "Execution timeout in milliseconds.",
          100,
          120000,
          30000,
        ),
        max_output_bytes: integer(
          "Maximum stdout and stderr bytes.",
          1024,
          512 * 1024,
          512 * 1024,
        ),
        index_output: boolean("Index large or truncated output.", true),
      },
      ["command"],
    ),
  },
  {
    name: "quill_search_context",
    label: "Search Quill context",
    description:
      "Search indexed working-context chunks and return bounded refs.",
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
  tool_execution_start: "PreToolUse",
  tool_execution_end: "PostToolUse",
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
export class UnknownSessionError extends QuillExtensionError {}
export class RegistrationError extends QuillExtensionError {}
export class PersistenceError extends QuillExtensionError {}

function errorMessage(error) {
  return error instanceof Error
    ? `${error.name}: ${error.message}`
    : `Error: ${String(error)}`;
}

function localBase(value) {
  // pi-lens-ignore: unchecked-throwing-call
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
    config = JSON.parse(
      readFileSync(join(root, ".config", "quill", "config.json"), "utf8"),
    );
  } catch (error) {
    throw new ConfigError("Quill config is missing or malformed", {
      cause: error,
    });
  }
  if (
    typeof config.url !== "string" ||
    typeof config.secret !== "string" ||
    !config.secret
  ) {
    throw new ConfigError("Invalid Quill config");
  }
  let main;
  let context;
  try {
    main = localBase(config.url);
    context = config.context_url
      ? localBase(config.context_url)
      : new URL(main);
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
    markerRoot: join(root, ".config", "quill", "context", "markers"),
    hostname:
      (typeof config.hostname === "string" && config.hostname.trim()) ||
      hostname().split(".")[0] ||
      "local",
    reporterContract:
      config.pi_reporter && typeof config.pi_reporter === "object"
        ? config.pi_reporter
        : undefined,
  };
}

function headers(config, includeReporter = false) {
  const value = {
    "Content-Type": "application/json",
    Authorization: `Bearer ${config.secret}`,
  };
  if (!includeReporter) return value;
  return {
    ...value,
    "X-Quill-Pi-Host": config.hostname.split(".")[0].toLowerCase(),
    "X-Quill-Pi-Process": reporterProcess(config).id,
    "X-Quill-Pi-Channel": config.installChannel || "unknown",
    "X-Quill-Pi-Protocol": String(PI_PROTOCOL_V2),
    "X-Quill-Pi-Reporter": PI_PROTOCOL_V2_REPORTER_VERSION,
    "X-Quill-Pi-Build": PI_PROTOCOL_V2_QUILL_BUILD,
    "X-Quill-Pi-Capability": PI_PROTOCOL_V2_CAPABILITY_DIGEST,
  };
}

function reporterNotice(root, code, message) {
  const notices = globalThis[REPORTER_NOTICES] || new Set();
  globalThis[REPORTER_NOTICES] = notices;
  const key = `${root}\u0000${code}`;
  if (notices.has(key)) return;
  notices.add(key);
  console.warn(`Quill Pi extension inactive: ${code}: ${message}`);
}

function pathInside(path, root) {
  const rel = relative(resolve(root), resolve(path));
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

export function reporterCandidateForPath(extensionPath, options = {}) {
  const home = options.home || process.env.HOME || homedir();
  const agentDir = resolve(
    options.agentDir || process.env.PI_CODING_AGENT_DIR || join(home, ".pi", "agent"),
  );
  const cwd = resolve(options.cwd || process.cwd());
  const path = resolve(extensionPath);
  const projectRoot = join(cwd, ".pi");
  let installChannel;
  if (path === join(agentDir, "extensions", "quill.ts")) {
    installChannel = "managed";
  } else if (
    pathInside(path, join(projectRoot, "extensions")) ||
    pathInside(path, join(projectRoot, "npm"))
  ) {
    installChannel = "project";
  } else if (
    path.endsWith(
      join("node_modules", "@sharaf-nassar", "quill-pi", "quill.ts"),
    )
  ) {
    installChannel = "npm";
  } else {
    installChannel = "development";
  }
  const selectedPath = options.selectedPath || process.env.QUILL_PI_REPORTER_PATH;
  const official = installChannel === "managed" || installChannel === "npm";
  return {
    install_channel: installChannel,
    extension_path: path,
    eligible: official || (selectedPath !== undefined && resolve(selectedPath) === path),
    protocol: PI_PROTOCOL_V2,
    reporter_version: PI_PROTOCOL_V2_REPORTER_VERSION,
    quill_build: PI_PROTOCOL_V2_QUILL_BUILD,
    capability_digest: PI_PROTOCOL_V2_CAPABILITY_DIGEST,
    capabilities: PI_PROTOCOL_V2_CAPABILITIES,
  };
}

function candidateMatchesContract(candidate, contract) {
  return (
    contract?.enabled === true &&
    candidate.eligible &&
    candidate.protocol === contract.protocol &&
    candidate.reporter_version === contract.reporter_version &&
    candidate.quill_build === contract.quill_build &&
    candidate.capability_digest === contract.capability_digest
  );
}

export function electReporterCandidate(candidates, contract) {
  return candidates
    .filter(
      (candidate) =>
        !candidate.failed && candidateMatchesContract(candidate, contract),
    )
    .sort(
      (left, right) =>
        REPORTER_CHANNEL_RANK[left.install_channel] -
          REPORTER_CHANNEL_RANK[right.install_channel] ||
        right.capabilities.length - left.capabilities.length ||
        left.extension_path.localeCompare(right.extension_path),
    )[0];
}

function resetReporterBroker(config, broker) {
  globalThis[REPORTER_BROKERS]?.delete(config.quillRoot);
  globalThis[REPORTER_CLAIMS]?.delete(config.quillRoot);
  broker.candidates.clear();
}

function installBrokerEvent(broker, event, config) {
  if (broker.registeredEvents.has(event)) return;
  broker.registeredEvents.add(event);
  try {
    broker.pi.on(event, (...args) => {
      const active = electReporterCandidate(
        [...broker.candidates.values()],
        broker.contract,
      );
      const handlers = active?.handlers.get(event) || [];
      const invoke = () => {
        let result;
        for (const handler of handlers) {
          const current = handler(...args);
          if (result === undefined && current !== undefined) result = current;
        }
        return result;
      };
      if (event !== "session_shutdown") return invoke();
      try {
        const result = invoke();
        if (result && typeof result.finally === "function") {
          return result.finally(() => resetReporterBroker(config, broker));
        }
        resetReporterBroker(config, broker);
        return result;
      } catch (error) {
        resetReporterBroker(config, broker);
        throw error;
      }
    });
  } catch (error) {
    writeLog(
      config,
      new RegistrationError(`Cannot register ${event}`, { cause: error }),
    );
  }
}

function installBrokerTool(broker, tool, config) {
  if (broker.registeredTools.has(tool.name)) return;
  broker.registeredTools.add(tool.name);
  try {
    broker.pi.registerTool({
      ...tool,
      async execute(...args) {
        const active = electReporterCandidate(
          [...broker.candidates.values()],
          broker.contract,
        );
        const execute = active?.tools.get(tool.name)?.execute;
        return execute ? execute(...args) : unavailable();
      },
    });
  } catch (error) {
    writeLog(
      config,
      new RegistrationError(`Cannot register ${tool.name}`, { cause: error }),
    );
  }
}

function registerReporterCandidate(pi, config, descriptor, setup) {
  const contract = config.reporterContract;
  if (contract?.enabled === false) return;
  if (!candidateMatchesContract(descriptor, contract)) {
    reporterNotice(
      config.quillRoot,
      descriptor.eligible
        ? "ReporterPairMismatch"
        : "ReporterOptInRequired",
      descriptor.eligible
        ? "install the exact Quill/reporter pair, then reload Pi"
        : `select ${descriptor.extension_path} with QUILL_PI_REPORTER_PATH, then reload Pi`,
    );
    return;
  }

  const brokers = globalThis[REPORTER_BROKERS] || new Map();
  const claims = globalThis[REPORTER_CLAIMS] || new Set();
  globalThis[REPORTER_BROKERS] = brokers;
  globalThis[REPORTER_CLAIMS] = claims;
  let broker = brokers.get(config.quillRoot);
  if (!broker && claims.has(config.quillRoot)) {
    reporterNotice(
      config.quillRoot,
      "ReporterReloadRequired",
      "a legacy pre-broker reporter is already loaded; remove it or reload Pi",
    );
    return;
  }
  if (!broker) {
    broker = {
      pi,
      contract,
      candidates: new Map(),
      registeredEvents: new Set(),
      registeredTools: new Set(),
    };
    brokers.set(config.quillRoot, broker);
    claims.add(config.quillRoot);
  }
  broker.contract = contract;

  const candidate = {
    ...descriptor,
    failed: false,
    handlers: new Map(),
    tools: new Map(),
  };
  const candidateApi = {
    appendEntry: (...args) => pi.appendEntry(...args),
    on(event, handler) {
      const handlers = candidate.handlers.get(event) || [];
      handlers.push(handler);
      candidate.handlers.set(event, handlers);
    },
    registerTool(tool) {
      candidate.tools.set(tool.name, tool);
    },
  };
  setup(candidateApi, {
    ...config,
    installChannel: descriptor.install_channel,
  });
  broker.candidates.set(candidate.extension_path, candidate);
  for (const event of candidate.handlers.keys()) {
    installBrokerEvent(broker, event, config);
  }
  installBrokerEvent(broker, "session_shutdown", config);
  for (const tool of candidate.tools.values()) {
    installBrokerTool(broker, tool, config);
  }
}

async function fetchJson(config, url, options) {
  const response = await fetch(url, {
    ...options,
    headers: headers(config),
    signal: AbortSignal.timeout(LOCAL_TIMEOUT_MS),
  });
  if (!response.ok)
    throw new TransportError(`Quill returned ${response.status}`);
  return response.json();
}

function trackedName(value) {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.trim() === value &&
    Buffer.byteLength(value) <= 256 &&
    !/[\u0000-\u001f\u007f-\u009f]/.test(value)
  );
}

function writeLog(_config, error) {
  console.error("Quill Pi extension", errorMessage(error));
}

function reporterProcess(config) {
  const processes = globalThis[REPORTER_PROCESS] || new Map();
  globalThis[REPORTER_PROCESS] = processes;
  let state = processes.get(config.quillRoot);
  if (!state) {
    state = { id: randomUUID(), origins: new Map(), sequence: 0 };
    processes.set(config.quillRoot, state);
  }
  return state;
}

async function responseError(response) {
  let body;
  try {
    body = await response.json();
  } catch {
    body = null;
  }
  const code = body?.code || body?.error;
  const message = body?.message || `Quill returned ${response.status}`;
  const typedMessage = code ? `${code}: ${message}` : message;
  if (response.status === 409 && code === "unknown_session") {
    return new UnknownSessionError(typedMessage);
  }
  if (response.status === 426 || code === "protocol_mismatch") {
    return new ProtocolMismatchError(typedMessage);
  }
  const error = new TransportError(typedMessage);
  error.status = response.status;
  error.retryAfterMs = Number.isFinite(body?.retry_after_ms)
    ? Math.max(0, Math.min(LOCAL_TIMEOUT_MS, body.retry_after_ms))
    : 0;
  return error;
}

function retryDelay(milliseconds) {
  return milliseconds > 0
    ? new Promise((resolve) => setTimeout(resolve, milliseconds))
    : Promise.resolve();
}

async function postPayload(config, endpoint, payload) {
  let authReloaded = false;
  let transientRetried = false;
  for (;;) {
    let response;
    try {
      response = await fetch(`${config.main}${endpoint}`, {
        method: "POST",
        headers: headers(config, true),
        body: JSON.stringify(payload),
        signal: AbortSignal.timeout(LOCAL_TIMEOUT_MS),
      });
    } catch (error) {
      if (!transientRetried) {
        transientRetried = true;
        continue;
      }
      throw new TransportError("Quill request failed", { cause: error });
    }
    if (response.status === 401 && !authReloaded) {
      authReloaded = true;
      Object.assign(config, loadConfig());
      continue;
    }
    if ([429, 503].includes(response.status) && !transientRetried) {
      transientRetried = true;
      const error = await responseError(response);
      await retryDelay(error.retryAfterMs);
      continue;
    }
    if (!response.ok) throw await responseError(response);
    try {
      await response.body?.cancel();
    } catch (error) {
      writeLog(
        config,
        new TransportError("Failed to release Quill response", {
          cause: error,
        }),
      );
    }
    return;
  }
}

async function sendTracked(config, state, endpoint, payload) {
  if (state.inert) return false;
  try {
    await postPayload(config, endpoint, payload);
    return true;
  } catch (error) {
    if (error instanceof UnknownSessionError && state.startEnvelope) {
      try {
        await postPayload(config, "/api/v1/pi/track", state.startEnvelope);
        if (payload !== state.startEnvelope) {
          await postPayload(config, endpoint, payload);
        }
        return true;
      } catch (recoveryError) {
        error = recoveryError;
      }
    }
    if (error instanceof ProtocolMismatchError) state.inert = true;
    writeLog(
      config,
      error instanceof QuillExtensionError
        ? error
        : new TransportError("Unexpected Pi tracking failure", {
            cause: error,
          }),
    );
    return false;
  }
}

function boundedText(value, maxBytes) {
  const text = typeof value === "string" ? value : String(value ?? "");
  const bytes = Buffer.from(text);
  if (bytes.length <= maxBytes) return text;
  return `${bytes
    .subarray(0, maxBytes)
    .toString("utf8")
    .replace(/\uFFFD$/, "")}... [truncated]`;
}

function compactHistory(data) {
  const source = data && typeof data === "object" ? data : {};
  const sourceHits = Array.isArray(source.hits) ? source.hits : [];
  const hits = [];
  let truncated = source.truncated === true;

  for (const sourceHit of sourceHits) {
    const hit = sourceHit && typeof sourceHit === "object" ? sourceHit : {};
    hits.push({
      provider: boundedText(hit.provider, 32),
      message_id: boundedText(hit.message_id, 512),
      session_id: boundedText(hit.session_id, 512),
      parent_session_id:
        hit.parent_session_id == null
          ? null
          : boundedText(hit.parent_session_id, 512),
      snippet: boundedText(hit.snippet, 2048),
      role: boundedText(hit.role, 32),
      project: boundedText(hit.project, 512),
      host: boundedText(hit.host, 512),
      timestamp: boundedText(hit.timestamp, 64),
      git_branch: boundedText(hit.git_branch, 512),
      score: typeof hit.score === "number" ? hit.score : 0,
    });
    const candidate = JSON.stringify({
      hits,
      total_hits:
        typeof source.total_hits === "number"
          ? source.total_hits
          : sourceHits.length,
      query_time_ms:
        typeof source.query_time_ms === "number" ? source.query_time_ms : 0,
      truncated: false,
    });
    if (Buffer.byteLength(candidate) > HISTORY_RESULT_MAX_BYTES) {
      hits.pop();
      truncated = true;
      break;
    }
  }

  if (hits.length < sourceHits.length) truncated = true;
  return {
    hits,
    total_hits:
      typeof source.total_hits === "number"
        ? source.total_hits
        : sourceHits.length,
    query_time_ms:
      typeof source.query_time_ms === "number" ? source.query_time_ms : 0,
    truncated,
  };
}

function success(data) {
  return {
    content: [{ type: "text", text: JSON.stringify(data) }],
    details: { ok: true },
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
  url.searchParams.set("view", "compact");
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
  return command.replace(
    /<<-?\s*["']?([A-Za-z0-9_]+)["']?[\s\S]*?\n\s*\1/g,
    "",
  );
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
    if (
      !/(^|\s)(curl|wget)\s/i.test(value) ||
      /\s(-I|--head)(\s|$)/.test(value)
    )
      return false;
    const curl = /\bcurl\b/i.test(value);
    const fileOutput = curl
      ? /\s(-o|--output)\s+\S+/.test(value) ||
        /\s(-O|--remote-name)(\s|$)/.test(value) ||
        /\s>>?\s*\S+/.test(value)
      : /\s(-O|--output-document)\s+\S+/.test(value) ||
        /\s>>?\s*\S+/.test(value);
    const quiet = curl
      ? /(^|\s)-[A-Za-z]*s[A-Za-z]*(\s|$)/.test(value) ||
        /\s--silent(\s|$)/.test(value)
      : /(^|\s)-[A-Za-z]*q[A-Za-z]*(\s|$)/.test(value) ||
        /\s--quiet(\s|$)/.test(value);
    const verbose = /\s(-v|--verbose|--trace|--trace-ascii|-D\s+-)(\s|$)/.test(
      value,
    );
    const stdout =
      /\s(-o|--output|-O|--output-document)\s+(-|\/dev\/stdout)(\s|$)/.test(
        value,
      );
    return !fileOutput || !quiet || verbose || stdout;
  });
}

function isInlineNetworkFetch(command) {
  const visible = stripHeredocs(command);
  return (
    /fetch\s*\(\s*["']https?:\/\//i.test(visible) ||
    /requests\.(get|post|put|patch)\s*\(/i.test(visible) ||
    /http\.(get|request)\s*\(/i.test(visible)
  );
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
    if (
      !/(?:^|\s)(?:curl|wget)(?:\s|$)/i.test(bare) ||
      /(?:^|\s)(?:-I|--head)(?:\s|$)/.test(bare)
    ) {
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
      if (path && !["-", "/dev/stdout", "/dev/null"].includes(path))
        paths.push(path);
    }
    const redirectPattern = new RegExp(`(?:^|\\s)>>?\\s*${outputTarget}`, "g");
    for (const match of segment.matchAll(redirectPattern)) {
      const path = unquoteToken(match[1]);
      if (path && !["/dev/stdout", "/dev/null"].includes(path))
        paths.push(path);
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
  const safeSession = String(sessionId || "unknown")
    .replace(/[^a-zA-Z0-9._-]+/g, "_")
    .slice(0, 120);
  return join(config.markerRoot, `pi-${safeSession}`, "tainted.json");
}

function loadTainted(config, sessionId) {
  try {
    const state = JSON.parse(
      readFileSync(taintedStatePath(config, sessionId), "utf8"),
    );
    return new Set(
      (Array.isArray(state.paths) ? state.paths : []).filter(
        (path) => !isDegenerateTaint(path),
      ),
    );
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
    writeLog(
      config,
      new QuillExtensionError("Taint persistence failed", { cause: error }),
    );
  }
}

function recordTainted(config, sessionId, cwd, paths) {
  if (!paths.length) return;
  const tainted = loadTainted(config, sessionId);
  for (const path of paths) {
    if (isDegenerateTaint(path)) continue;
    tainted.add(path);
    const resolved = resolveLiteralPath(config, cwd, path);
    if (resolved && resolved !== path && !isDegenerateTaint(resolved))
      tainted.add(resolved);
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
    const pattern = new RegExp(
      `(?:^|[\\s=])${escapeRegExp(path)}(?:[\\s)>;|&]|$)`,
    );
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
  return (
    !BINARY_URL_EXT_RE.test(url) &&
    (/^https?:\/\/api\./i.test(url) ||
      /[?&]format=json|\.json(\?|$)|\/api\//i.test(url))
  );
}

function fetchDenyReason(command, explicitUrls = []) {
  const urls = explicitUrls.length ? explicitUrls : extractFetchUrls(command);
  const lines = ["Quill context routing blocked a raw network fetch."];
  if (urls.length) {
    lines.push("", "Run this instead:");
    for (const url of urls) {
      lines.push(`  quill_fetch_and_index(url=${JSON.stringify(url)})`);
      if (looksLikeApiJson(url)) {
        lines.push(
          `  quill_execute(command=${JSON.stringify(`curl -sS ${url} | jq .`)})`,
        );
      }
    }
    lines.push("", "Then use quill_search_context to retrieve focused chunks.");
  } else {
    lines.push(
      "Use quill_execute for a bounded curl and jq workflow, or quill_fetch_and_index for pages.",
    );
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
      headers: headers(config, Boolean(sessionInfo(ctx).file)),
      body: JSON.stringify({ events: [body] }),
      signal: AbortSignal.timeout(LOCAL_TIMEOUT_MS),
    })
      .then((response) =>
        consumeTelemetryResponse(config, response, "routing telemetry"),
      )
      .catch((error) =>
        writeLog(
          config,
          error instanceof QuillExtensionError
            ? error
            : new TransportError("Routing telemetry failed", { cause: error }),
        ),
      );
  } catch (error) {
    writeLog(
      config,
      new TransportError("Routing telemetry failed", { cause: error }),
    );
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
  if (
    ["fetch", "web_fetch", "webfetch", "fetch_content"].includes(event.toolName)
  ) {
    const urls = Array.isArray(input.urls)
      ? input.urls.filter((url) => typeof url === "string")
      : [];
    if (!urls.length && typeof input.url === "string") urls.push(input.url);
    return deny(config, event, ctx, fetchDenyReason("", urls), "webfetch");
  }
  if (event.toolName === "bash") {
    const command = typeof input.command === "string" ? input.command : "";
    if (!command) return undefined;
    if (hasRawNetworkDump(command) || isInlineNetworkFetch(command)) {
      return deny(
        config,
        event,
        ctx,
        fetchDenyReason(command),
        "raw-network-fetch",
      );
    }
    const tainted = loadTainted(config, sessionId);
    const hit = commandReadsTaintedPath(command, tainted);
    if (hit)
      return deny(
        config,
        event,
        ctx,
        taintedReadDenyReason("bash", hit),
        "tainted-read-bash",
      );
    recordTainted(config, sessionId, ctx.cwd, extractFetchOutputPaths(command));
    return undefined;
  }
  if (event.toolName === "read") {
    const path = typeof input.path === "string" ? input.path : "";
    const hit = readTargetsTaintedPath(
      config,
      ctx.cwd,
      path,
      loadTainted(config, sessionId),
    );
    if (hit)
      return deny(
        config,
        event,
        ctx,
        taintedReadDenyReason("read", hit),
        "tainted-read",
      );
  }
  return undefined;
}

async function consumeTelemetryResponse(config, response, label) {
  if (response.ok === false) throw await responseError(response);
  try {
    await response.body?.cancel();
  } catch (error) {
    writeLog(config, new TransportError(`Failed to release ${label} response`, { cause: error }));
  }
}

function postTelemetry(config, event, ctx, hookEvent) {
  if (!hookEvent) return;
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
    headers: headers(config, true),
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(LOCAL_TIMEOUT_MS),
  })
    .then((response) => consumeTelemetryResponse(config, response, "hook telemetry"))
    .catch((error) =>
      writeLog(
        config,
        error instanceof QuillExtensionError
          ? error
          : new TransportError("Hook telemetry failed", { cause: error }),
      ),
    );
}

function isoTimestamp(value) {
  const date = value === undefined ? new Date() : new Date(value);
  return Number.isNaN(date.valueOf())
    ? new Date().toISOString()
    : date.toISOString();
}

function stableId(prefix, ...parts) {
  return `${prefix}_${createHash("sha256").update(JSON.stringify(parts)).digest("hex").slice(0, 32)}`;
}

function stableUuid(...parts) {
  const hex = createHash("sha256")
    .update(JSON.stringify(parts))
    .digest("hex")
    .slice(0, 32)
    .split("");
  hex[12] = "4";
  hex[16] = ["8", "9", "a", "b"][Number.parseInt(hex[16], 16) % 4];
  return `${hex.slice(0, 8).join("")}-${hex.slice(8, 12).join("")}-${hex.slice(12, 16).join("")}-${hex.slice(16, 20).join("")}-${hex.slice(20).join("")}`;
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
    if (
      header?.type !== "session" ||
      typeof header.id !== "string" ||
      !header.id
    ) {
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
  const parentPath =
    info.header?.parentSession ||
    (event.reason === "fork" ? event.previousSessionFile : undefined);
  const subagent = process.env.PI_SUBAGENT_CHILD === "1";
  const resolved = new Map();
  const resolveId = (path) => {
    if (!resolved.has(path)) resolved.set(path, readSessionId(path));
    return resolved.get(path);
  };
  let lineage = { kind: "root" };
  let parentSessionId;
  if (parentPath) {
    try {
      const headerParent = resolveId(parentPath);
      if (trackedName(headerParent) && headerParent !== info.id)
        parentSessionId = headerParent;
    } catch {}
  }
  const envParent = process.env.PI_SUBAGENT_PARENT_SESSION;
  if (
    !parentSessionId &&
    subagent &&
    trackedName(envParent) &&
    envParent !== info.id
  ) {
    parentSessionId = envParent;
  }
  if (parentSessionId) {
    lineage = {
      kind: subagent ? "agent" : "linked",
      parent_session_id: parentSessionId,
    };
  } else if (parentPath || subagent) {
    lineage = {
      kind: "unresolved",
      reason: subagent
        ? "subagent_parent_unavailable"
        : "parent_header_unavailable",
    };
  }
  let previousSessionId;
  if (
    ["new", "resume", "fork"].includes(event.reason) &&
    event.previousSessionFile
  ) {
    try {
      previousSessionId = resolveId(event.previousSessionFile);
    } catch (error) {
      previousSessionId = undefined;
    }
  }
  return { lineage, previousSessionId };
}

export function buildProtocolV2Event(fields) {
  const event = {
    event_uuid: fields.event_uuid,
    event: fields.event,
    provider: fields.provider ?? "pi",
    normalized_host: fields.normalized_host,
    session_id: fields.session_id,
    process_instance_id: fields.process_instance_id,
    sequence: fields.sequence,
    origin_at: fields.origin_at,
    occurred_at: fields.occurred_at,
    delivery_source: fields.delivery_source,
  };
  if (fields.event === "session_start") {
    event.reason = fields.reason;
    if (fields.previous_session_id !== undefined)
      event.previous_session_id = fields.previous_session_id;
    event.lineage = fields.lineage;
    if (fields.agent_role !== undefined) event.agent_role = fields.agent_role;
  } else if (fields.event === "session_end") {
    event.reason = fields.reason;
  } else if (fields.event === "lineage") {
    event.lineage = fields.lineage;
    if (fields.agent_role !== undefined) event.agent_role = fields.agent_role;
  }
  return event;
}

export function buildProtocolV2Envelope(events, generation = {}) {
  return {
    protocol: generation.protocol ?? PI_PROTOCOL_V2,
    reporter_version:
      generation.reporter_version ?? PI_PROTOCOL_V2_REPORTER_VERSION,
    quill_build: generation.quill_build ?? PI_PROTOCOL_V2_QUILL_BUILD,
    capability_digest:
      generation.capability_digest ?? PI_PROTOCOL_V2_CAPABILITY_DIGEST,
    events,
  };
}

export function buildQuillTrackingEntry(event, generation = {}) {
  return {
    type: "custom",
    customType: "quill-tracking",
    data: {
      schema: generation.schema ?? PI_PROTOCOL_V2_TRACKING_SCHEMA,
      ...event,
      reporter: {
        protocol: generation.protocol ?? PI_PROTOCOL_V2,
        version: generation.reporter_version ?? PI_PROTOCOL_V2_REPORTER_VERSION,
        quill_build: generation.quill_build ?? PI_PROTOCOL_V2_QUILL_BUILD,
        capability_digest:
          generation.capability_digest ?? PI_PROTOCOL_V2_CAPABILITY_DIGEST,
      },
    },
  };
}

function fixtureEvent(index, fields = {}) {
  return buildProtocolV2Event({
    event_uuid: `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
    event: "session_start",
    normalized_host: "pi-host",
    session_id: "session-root",
    process_instance_id: "10000000-0000-4000-8000-000000000001",
    sequence: index,
    origin_at: "2026-08-18T02:00:00.000Z",
    occurred_at: `2026-08-18T02:00:${String(index).padStart(2, "0")}.000Z`,
    delivery_source: "live",
    reason: "startup",
    lineage: { kind: "root" },
    ...fields,
  });
}

export function protocolV2FixtureJsonl() {
  const cases = [];
  const add = (name, kind, expectation, coverage, value, error_code) => {
    cases.push({
      name,
      kind,
      expectation,
      coverage,
      ...(error_code ? { error_code } : {}),
      wire: JSON.stringify(value),
    });
  };

  const startCases = [
    ["startup", "live", { kind: "root" }, {}],
    [
      "reload",
      "live",
      { kind: "linked", parent_session_id: "session-parent" },
      {},
    ],
    [
      "new",
      "live",
      { kind: "agent", parent_session_id: "session-parent" },
      { previous_session_id: "session-old", agent_role: "reviewer" },
    ],
    [
      "resume",
      "reconciliation",
      { kind: "unresolved", reason: "subagent_parent_unavailable" },
      { previous_session_id: "session-old", agent_role: "researcher" },
    ],
    [
      "fork",
      "reconciliation",
      { kind: "linked", parent_session_id: "session-parent" },
      { previous_session_id: "session-old" },
    ],
  ];
  startCases.forEach(([reason, delivery_source, lineage, options], offset) => {
    const event = fixtureEvent(offset + 1, {
      reason,
      delivery_source,
      lineage,
      ...options,
    });
    add(
      `envelope.start.${reason}`,
      "envelope",
      "accept",
      [
        `start:${reason}`,
        `delivery:${delivery_source}`,
        `lineage:${lineage.kind}`,
        `option:previous_session_id:${options.previous_session_id ? "present" : "omitted"}`,
        `option:agent_role:${options.agent_role ? "present" : "omitted"}`,
      ],
      buildProtocolV2Envelope([event]),
    );
  });

  ["quit", "reload", "new", "resume", "fork"].forEach((reason, offset) => {
    const event = fixtureEvent(offset + 6, {
      event: "session_end",
      reason,
      delivery_source: offset === 4 ? "reconciliation" : "live",
    });
    add(
      `envelope.end.${reason}`,
      "envelope",
      "accept",
      [`end:${reason}`, `delivery:${event.delivery_source}`],
      buildProtocolV2Envelope([event]),
    );
  });

  const lineageEvents = [
    { kind: "root" },
    { kind: "linked", parent_session_id: "session-parent" },
    { kind: "agent", parent_session_id: "session-parent" },
    { kind: "unresolved", reason: "parent_header_unavailable" },
  ];
  lineageEvents.forEach((lineage, offset) => {
    const event = fixtureEvent(offset + 11, {
      event: "lineage",
      lineage,
      ...(lineage.kind === "agent" ? { agent_role: "reviewer" } : {}),
    });
    add(
      `envelope.lineage.${lineage.kind}`,
      "envelope",
      "accept",
      [
        `lineage:${lineage.kind}`,
        `option:agent_role:${event.agent_role ? "present" : "omitted"}`,
      ],
      buildProtocolV2Envelope([event]),
    );
  });

  const entryEvent = fixtureEvent(15, {
    reason: "new",
    lineage: { kind: "agent", parent_session_id: "session-parent" },
    previous_session_id: "session-old",
    agent_role: "reviewer",
  });
  add(
    "entry.valid",
    "entry",
    "accept",
    ["entry:schema:2", "option:agent_role:present"],
    buildQuillTrackingEntry(entryEvent),
  );

  const invalidEnvelope = buildProtocolV2Envelope([fixtureEvent(16)]);
  invalidEnvelope.unexpected = true;
  add(
    "envelope.invalid_field",
    "envelope",
    "reject",
    ["invalid:field", "invalid:envelope_field"],
    invalidEnvelope,
    "invalid_envelope",
  );
  const invalidEvent = { ...fixtureEvent(17), unexpected: true };
  add(
    "event.invalid_field",
    "envelope",
    "reject",
    ["invalid:field", "invalid:event_field"],
    buildProtocolV2Envelope([invalidEvent]),
    "invalid_event",
  );
  const invalidLineageEvent = fixtureEvent(18, {
    lineage: { kind: "root", unexpected: true },
  });
  add(
    "lineage.invalid_field",
    "envelope",
    "reject",
    ["invalid:field", "invalid:lineage_field"],
    buildProtocolV2Envelope([invalidLineageEvent]),
    "invalid_event",
  );
  const nullOptionEvent = fixtureEvent(19, {
    reason: "new",
    previous_session_id: null,
  });
  add(
    "event.null_option",
    "envelope",
    "reject",
    ["invalid:field", "invalid:optional_null"],
    buildProtocolV2Envelope([nullOptionEvent]),
    "invalid_event",
  );

  for (const [name, generation, coverage, error] of [
    [
      "protocol_older",
      { protocol: 1 },
      "mismatch:protocol:older",
      "protocol_mismatch",
    ],
    [
      "protocol_newer",
      { protocol: 3 },
      "mismatch:protocol:newer",
      "protocol_mismatch",
    ],
    [
      "reporter_version",
      { reporter_version: "0.0.9" },
      "mismatch:reporter_version",
      "reporter_version_mismatch",
    ],
    [
      "quill_build",
      { quill_build: "0.0.1" },
      "mismatch:quill_build",
      "quill_build_mismatch",
    ],
    [
      "capability_digest",
      { capability_digest: "0".repeat(64) },
      "mismatch:capability_digest",
      "capability_mismatch",
    ],
  ]) {
    add(
      `envelope.${name}`,
      "envelope",
      "reject",
      [coverage],
      buildProtocolV2Envelope([fixtureEvent(20)], generation),
      error,
    );
  }

  for (const [schema, direction] of [
    [1, "older"],
    [3, "newer"],
  ]) {
    add(
      `entry.schema_${direction}`,
      "entry",
      "reject",
      [`mismatch:schema:${direction}`],
      buildQuillTrackingEntry(entryEvent, { schema }),
      "tracking_schema_mismatch",
    );
  }

  add(
    "response.accepted",
    "response",
    "accept",
    [
      "handshake:accepted",
      "outcome:applied",
      "outcome:duplicate",
      "outcome:stale",
    ],
    {
      status: "accepted",
      quill_build: PI_PROTOCOL_V2_QUILL_BUILD,
      protocol: PI_PROTOCOL_V2,
      reporter_version: PI_PROTOCOL_V2_REPORTER_VERSION,
      capability_digest: PI_PROTOCOL_V2_CAPABILITY_DIGEST,
      outcomes: ["applied", "duplicate", "stale"],
    },
  );
  add(
    "response.unknown_session",
    "response",
    "accept",
    [
      "outcome:unknown_session",
      "response:409",
      "option:required:omitted",
      "option:retry_after_ms:omitted",
    ],
    {
      status: "error",
      code: "unknown_session",
      message: "Session lifecycle must be reannounced",
    },
  );
  add(
    "response.exact_mismatch",
    "response",
    "accept",
    [
      "response:426",
      "option:required:present",
      "option:retry_after_ms:omitted",
    ],
    {
      status: "error",
      code: "protocol_mismatch",
      message: "Install the exact Quill and reporter pair",
      required: {
        protocol: PI_PROTOCOL_V2,
        reporter_version: PI_PROTOCOL_V2_REPORTER_VERSION,
        quill_build: PI_PROTOCOL_V2_QUILL_BUILD,
        capability_digest: PI_PROTOCOL_V2_CAPABILITY_DIGEST,
      },
    },
  );
  add(
    "response.rate_limited",
    "response",
    "accept",
    [
      "response:429",
      "option:required:omitted",
      "option:retry_after_ms:present",
    ],
    {
      status: "error",
      code: "rate_limited",
      message: "Retry after the bounded delay",
      retry_after_ms: 1500,
    },
  );

  return `${cases.map((entry) => JSON.stringify(entry)).join("\n")}\n`;
}

function trackEnvelope(events) {
  return {
    protocol: PROTOCOL_VERSION,
    extension_version: EXTENSION_VERSION,
    min_quill_version: MIN_QUILL_VERSION,
    events,
  };
}

function persistLifecycle(config, state, info, type, fields) {
  const sequence = ++state.process.sequence;
  let originAt = state.process.origins.get(info.id);
  if (!originAt) {
    originAt = isoTimestamp(info.header?.timestamp);
    state.process.origins.set(info.id, originAt);
  }
  const event = buildProtocolV2Event({
    event_uuid: stableUuid(
      state.process.id,
      info.id,
      sequence,
      type,
      fields.reason,
    ),
    event: type,
    normalized_host: config.hostname.split(".")[0].toLowerCase(),
    session_id: info.id,
    process_instance_id: state.process.id,
    sequence,
    origin_at: originAt,
    occurred_at: fields.occurred_at || isoTimestamp(),
    delivery_source: "live",
    reason: fields.reason,
    ...(fields.previous_session_id
      ? { previous_session_id: fields.previous_session_id }
      : {}),
    ...(fields.lineage ? { lineage: fields.lineage } : {}),
  });
  const entry = buildQuillTrackingEntry(event);
  try {
    state.pi.appendEntry(entry.customType, entry.data);
  } catch (error) {
    writeLog(
      config,
      new PersistenceError("Cannot append Pi tracking entry", { cause: error }),
    );
    return Promise.resolve(false);
  }
  const envelope = buildProtocolV2Envelope([event]);
  if (type === "session_start") state.startEnvelope = envelope;
  return sendTracked(config, state, "/api/v1/pi/track", envelope);
}

function trackEvent(
  config,
  state,
  info,
  type,
  fields = {},
  identity = randomUUID(),
) {
  if (!info.file) return Promise.resolve(true);
  if (type === "session_start" || type === "session_end") {
    return persistLifecycle(config, state, info, type, fields);
  }
  const timestamp = fields.timestamp || isoTimestamp();
  const event = {
    type,
    event_uuid: stableId("pi", info.id, type, identity),
    session_id: info.id,
    hostname: config.hostname,
    timestamp,
    ...fields,
  };
  return sendTracked(config, state, "/api/v1/pi/track", trackEnvelope([event]));
}

function trackEvents(config, state, info, events) {
  if (!info.file) return Promise.resolve(true);
  return sendTracked(config, state, "/api/v1/pi/track", trackEnvelope(events));
}

function runtimeMessage(config, state, info, message) {
  if (!info.file) return Promise.resolve(true);
  const payload = {
    provider: "pi",
    host: config.hostname,
    session_id: info.id,
    project: info.cwd || "unknown",
    cwd: info.cwd,
    messages: [
      {
        uuid: message.uuid,
        type: message.type,
        timestamp: message.timestamp,
        content: "",
        role: message.role,
        tools_used: message.tools || [],
        files_modified: [],
        event_kinds: message.eventKinds,
      },
    ],
  };
  return sendTracked(config, state, "/api/v1/sessions/messages", payload);
}

function notifySession(config, state, info, lineage) {
  // Pi names the transcript at session start but only writes it once the first
  // assistant message lands, so the path exists before the file does. Notifying
  // the named-but-absent path is a guaranteed rejection; turn end repeats the
  // notify with the same identity once the transcript is on disk.
  if (!info.file || !existsSync(info.file)) return Promise.resolve(true);
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
      writeLog(
        config,
        new TransportError("Pi tracking handler failed", { cause: error }),
      );
    }
  });
}

function registerHandler(pi, config, event, handler) {
  try {
    pi.on(event, handler);
  } catch (error) {
    writeLog(
      config,
      new RegistrationError(`Cannot register ${event}`, { cause: error }),
    );
  }
}

function registerTracking(pi, config, trackingOnlyChild) {
  const state = {
    assistantTextMessages: new Set(),
    inert: false,
    notify: null,
    pi,
    process: reporterProcess(config),
    startEnvelope: null,
  };
  const activity = (event, ctx, name, hookEvent) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      if (!info.file) return;
      postTelemetry(config, event, ctx, hookEvent);
      return trackEvent(
        config,
        state,
        info,
        "activity",
        {},
        `${name}:${randomUUID()}`,
      );
    });
  };

  registerHandler(pi, config, "session_start", (event, ctx) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      if (!info.file) return;
      const { lineage, previousSessionId } = resolveStart(event, info);
      state.notify = { info, lineage };
      postTelemetry(config, event, ctx, EVENT_MAP.session_start);
      const handshake = trackEvent(config, state, info, "session_start", {
        reason: event.reason,
        ...(previousSessionId
          ? { previous_session_id: previousSessionId }
          : {}),
        lineage,
      });
      void notifySession(config, state, info, lineage);
      return handshake;
    });
  });
  registerHandler(pi, config, "session_shutdown", async (event, ctx) => {
    try {
      const info = sessionInfo(ctx);
      if (!info.file) return;
      postTelemetry(config, event, ctx, EVENT_MAP.session_shutdown);
      await trackEvent(config, state, info, "session_end", {
        reason: event.reason,
      });
    } catch (error) {
      writeLog(
        config,
        new TransportError("Pi shutdown tracking failed", { cause: error }),
      );
    }
  });
  registerHandler(pi, config, "agent_start", (event, ctx) =>
    activity(
      event,
      ctx,
      "agent_start",
      trackingOnlyChild ? "SubagentStart" : undefined,
    ),
  );
  registerHandler(pi, config, "agent_settled", (event, ctx) =>
    activity(
      event,
      ctx,
      "agent_settled",
      trackingOnlyChild ? "SubagentStop" : "Stop",
    ),
  );
  registerHandler(pi, config, "turn_start", (event, ctx) =>
    activity(event, ctx, "turn_start"),
  );
  registerHandler(pi, config, "input", (event, ctx) => {
    defer(config, () => {
      const info = sessionInfo(ctx);
      if (!info.file) return;
      const timestamp = isoTimestamp();
      postTelemetry(config, event, ctx, EVENT_MAP.input);
      void trackEvent(
        config,
        state,
        info,
        "activity",
        { timestamp },
        `input:${randomUUID()}`,
      );
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
      if (!info.file) return;
      const timestamp = isoTimestamp();
      postTelemetry(config, event, ctx, EVENT_MAP.tool_execution_start);
      void trackEvent(
        config,
        state,
        info,
        "activity",
        { timestamp },
        `tool-start:${event.toolCallId}`,
      );
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
      if (!info.file) return;
      const timestamp = isoTimestamp();
      postTelemetry(config, event, ctx, EVENT_MAP.tool_execution_end);
      void trackEvent(
        config,
        state,
        info,
        "activity",
        { timestamp },
        `tool-end:${event.toolCallId}`,
      );
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
      if (!info.file) return;
      return trackEvent(
        config,
        state,
        info,
        "model",
        {
          model_provider: event.model.provider,
          model: event.model.id,
        },
        `model:${event.model.provider}:${event.model.id}:${randomUUID()}`,
      );
    });
  });
  registerHandler(pi, config, "message_end", (event, ctx) => {
    if (event.message?.role !== "assistant") return;
    defer(config, () => {
      const info = sessionInfo(ctx);
      if (!info.file) return;
      const message = event.message;
      const timestamp = isoTimestamp(message.timestamp);
      const identity =
        message.responseId ||
        stableId(
          "response",
          info.id,
          timestamp,
          message.provider,
          message.model,
        );
      const hasText = message.content?.some(
        (part) =>
          part?.type === "text" &&
          typeof part.text === "string" &&
          part.text.trim().length > 0,
      );
      if (hasText && !state.assistantTextMessages.has(identity)) {
        state.assistantTextMessages.add(identity);
        void runtimeMessage(config, state, info, {
          uuid: stableId("pi_msg", info.id, "text", identity),
          type: "assistant",
          timestamp,
          role: "assistant",
          eventKinds: ["asst_text"],
        });
      }
      if (!message.usage) return;
      const cost = message.usage.cost || {};
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
      if (!info.file) return;
      const timestamp = isoTimestamp(event.message?.timestamp);
      void trackEvent(
        config,
        state,
        info,
        "activity",
        { timestamp },
        `turn:${event.turnIndex}`,
      );
      if (state.notify) {
        void notifySession(
          config,
          state,
          state.notify.info,
          state.notify.lineage,
        );
      }
      return undefined;
    });
  });
}

function configureReporter(pi, config) {
  const trackingOnlyChild = process.env.PI_SUBAGENT_CHILD === "1";
  if (FEATURES.context_preservation && !trackingOnlyChild) {
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
                return success(
                  compactHistory(
                    await fetchJson(config, historyUrl(config, params), {
                      method: "GET",
                    }),
                  ),
                );
              }
              const payload = { ...params };
              if (tool.withCwd && !payload.cwd) payload.cwd = ctx.cwd;
              const data = await fetchJson(
                config,
                `${config.context}/api/v1/context/${tool.endpoint}`,
                {
                  method: "POST",
                  body: JSON.stringify(payload),
                },
              );
              return success(data);
            } catch (error) {
              return unavailable();
            }
          },
        });
      } catch (error) {
        writeLog(
          config,
          new RegistrationError(`Cannot register ${tool.name}`, {
            cause: error,
          }),
        );
      }
    }
    registerHandler(pi, config, "tool_call", (event, ctx) => {
      try {
        return routeToolCall(config, event, ctx);
      } catch (error) {
        writeLog(
          config,
          new QuillExtensionError("Context routing failed", { cause: error }),
        );
        return undefined;
      }
    });
  }

  if (FEATURES.activity_tracking) {
    registerTracking(pi, config, trackingOnlyChild);
    if (!trackingOnlyChild) {
      for (const [eventName, hookEvent] of Object.entries(EVENT_MAP)) {
        if (TRACKING_EVENTS.has(eventName)) continue;
        registerHandler(pi, config, eventName, (event, ctx) => {
          try {
            if (!sessionInfo(ctx).file) return;
            postTelemetry(config, event, ctx, hookEvent);
          } catch (error) {
            writeLog(
              config,
              new TransportError("Hook telemetry failed", { cause: error }),
            );
          }
        });
      }
    }
  }
}

// @lat: [[infrastructure#Infrastructure#Pi Integration Deployment#Extension Tools and Telemetry]]
export default function quill(pi) {
  let config;
  try {
    config = loadConfig();
  } catch (error) {
    const root = process.env.HOME || process.env.USERPROFILE || homedir();
    reporterNotice(root, "ConfigError", "install or repair Quill config");
    return;
  }
  const descriptor = reporterCandidateForPath(EXTENSION_PATH, {
    home: config.home,
  });
  registerReporterCandidate(pi, config, descriptor, configureReporter);
}
