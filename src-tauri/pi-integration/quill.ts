// quill-managed:pi
// quill-managed-pi-payload: 2
// Quill-managed Pi integration, payload/stamp 2.
// Disable Pi in Quill to remove this file.

import { readFileSync } from "node:fs";
import { hostname, homedir } from "node:os";
import { join } from "node:path";

// Keep equal to LOCAL_TIMEOUT_MS in ../codex-integration/scripts/lib.cjs.
const LOCAL_TIMEOUT_MS = 1500;
const CONTEXT_PORT = "19877";
const FEATURES = { context_preservation: true, activity_tracking: true };

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
