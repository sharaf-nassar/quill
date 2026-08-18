import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import quill, {
  PI_PROTOCOL_V2,
  PI_PROTOCOL_V2_CAPABILITIES,
  PI_PROTOCOL_V2_CAPABILITY_DIGEST,
  PI_PROTOCOL_V2_QUILL_BUILD,
  PI_PROTOCOL_V2_REPORTER_VERSION,
  buildProtocolV2Envelope,
  electReporterCandidate,
  reporterCandidateForPath,
  buildProtocolV2Event,
  buildQuillTrackingEntry,
  protocolV2FixtureJsonl,
} from "./quill.ts";

const TOOL_NAMES = [
  "quill_search_history",
  "quill_index_context",
  "quill_fetch_and_index",
  "quill_execute",
  "quill_search_context",
  "quill_get_context_source",
  "quill_context_stats",
  "quill_purge_context",
];

function configRoot(config) {
  const root = mkdtempSync(join(tmpdir(), "quill-pi-extension-"));
  const path = join(root, ".config", "quill", "config.json");
  mkdirSync(dirname(path), { recursive: true });
  if (config !== undefined) {
    writeFileSync(
      path,
      typeof config === "string" ? config : JSON.stringify(config),
    );
  }
  return root;
}

function fakePi({ registerError, onAppend } = {}) {
  const tools = new Map();
  const handlers = new Map();
  const entries = [];
  let registrationAttempts = 0;
  return {
    api: {
      appendEntry(customType, data) {
        entries.push({ customType, data });
        onAppend?.(customType, data);
      },
      registerTool(tool) {
        registrationAttempts += 1;
        if (registerError) throw registerError;
        tools.set(tool.name, tool);
      },
      on(event, handler) {
        const registered = handlers.get(event) || [];
        registered.push(handler);
        handlers.set(event, registered);
      },
    },
    entries,
    handlers,
    tools,
    registrationAttempts: () => registrationAttempts,
  };
}

function allHandlers(pi) {
  return [...pi.handlers.values()].flat();
}

function routingHandler(pi) {
  return pi.handlers.get("tool_call")?.[0];
}

function context(sessionId = "pi-session", options = {}) {
  const sessionFile = Object.hasOwn(options, "sessionFile")
    ? options.sessionFile
    : join(process.env.HOME || tmpdir(), `${sessionId}.jsonl`);
  return {
    cwd: "/tmp/project",
    mode: "tui",
    ui: { notify: options.notify || (() => {}) },
    model: options.model,
    sessionManager: {
      getSessionId: () => sessionId,
      getSessionFile: () => sessionFile,
      getHeader: () => ({
        type: "session",
        id: sessionId,
        timestamp: "2026-08-14T08:00:00.000Z",
        cwd: "/tmp/project",
        ...(options.parentSession
          ? { parentSession: options.parentSession }
          : {}),
      }),
    },
  };
}

// Mirrors `features_declaration` in src-tauri/src/integrations/pi.rs: match the
// declaration, not its layout, so a formatter rewrapping quill.ts cannot turn
// feature rendering into a silent no-op.
function renderFeatures(source, flags) {
  const start = source.indexOf("const FEATURES");
  const end = source.indexOf("};", start) + 2;
  assert.ok(start !== -1 && end > start, "payload declares const FEATURES");
  return (
    source.slice(0, start) +
    `const FEATURES = { context_preservation: ${flags.context_preservation}, activity_tracking: ${flags.activity_tracking}, context_telemetry: ${flags.context_telemetry} };` +
    source.slice(end)
  );
}

async function flushRequests() {
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
}

function httpResponse(status, body = {}) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    body: { cancel: async () => {} },
  };
}

const SOURCE_PATH = fileURLToPath(new URL("./quill.ts", import.meta.url));
const REPORTER_CONTRACT = {
  enabled: true,
  protocol: PI_PROTOCOL_V2,
  reporter_version: PI_PROTOCOL_V2_REPORTER_VERSION,
  quill_build: PI_PROTOCOL_V2_QUILL_BUILD,
  capability_digest: PI_PROTOCOL_V2_CAPABILITY_DIGEST,
};

async function withHome(config, run) {
  const configured =
    config && typeof config === "object"
      ? { pi_reporter: REPORTER_CONTRACT, ...config }
      : config;
  const root = configRoot(configured);
  const oldHome = process.env.HOME;
  const oldChild = process.env.PI_SUBAGENT_CHILD;
  const oldParent = process.env.PI_SUBAGENT_PARENT_SESSION;
  const oldReporterPath = process.env.QUILL_PI_REPORTER_PATH;
  process.env.HOME = root;
  process.env.QUILL_PI_REPORTER_PATH = SOURCE_PATH;
  delete process.env.PI_SUBAGENT_CHILD;
  delete process.env.PI_SUBAGENT_PARENT_SESSION;
  try {
    return await run(root);
  } finally {
    if (oldHome === undefined) delete process.env.HOME;
    else process.env.HOME = oldHome;
    if (oldChild === undefined) delete process.env.PI_SUBAGENT_CHILD;
    else process.env.PI_SUBAGENT_CHILD = oldChild;
    if (oldParent === undefined) delete process.env.PI_SUBAGENT_PARENT_SESSION;
    else process.env.PI_SUBAGENT_PARENT_SESSION = oldParent;
    if (oldReporterPath === undefined) delete process.env.QUILL_PI_REPORTER_PATH;
    else process.env.QUILL_PI_REPORTER_PATH = oldReporterPath;
    rmSync(root, { recursive: true, force: true });
  }
}

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Protocol v2 fixture contract]]
test("protocol v2 fixture is deterministic and privacy-safe", () => {
  const fixturePath = join(
    dirname(new URL(import.meta.url).pathname),
    "fixtures",
    "protocol-v2.jsonl",
  );
  const fixture = readFileSync(fixturePath, "utf8");
  assert.equal(protocolV2FixtureJsonl(), fixture);

  const cases = fixture
    .trimEnd()
    .split("\n")
    .map((line) => JSON.parse(line));
  const coverage = new Set(cases.flatMap((entry) => entry.coverage));
  for (const value of [
    ...["startup", "reload", "new", "resume", "fork"].map(
      (reason) => `start:${reason}`,
    ),
    ...["quit", "reload", "new", "resume", "fork"].map(
      (reason) => `end:${reason}`,
    ),
    "delivery:live",
    "delivery:reconciliation",
    "lineage:root",
    "lineage:linked",
    "lineage:agent",
    "lineage:unresolved",
    "option:previous_session_id:omitted",
    "option:previous_session_id:present",
    "option:agent_role:omitted",
    "option:agent_role:present",
    "option:required:omitted",
    "option:required:present",
    "option:retry_after_ms:omitted",
    "option:retry_after_ms:present",
    "invalid:field",
    "mismatch:protocol:older",
    "mismatch:protocol:newer",
    "mismatch:reporter_version",
    "mismatch:quill_build",
    "mismatch:capability_digest",
    "outcome:applied",
    "outcome:duplicate",
    "outcome:stale",
    "outcome:unknown_session",
    "handshake:accepted",
  ]) {
    assert.ok(coverage.has(value), value);
  }

  for (const entry of cases.filter(
    ({ kind, expectation }) => kind === "entry" && expectation === "accept",
  )) {
    assert.doesNotMatch(entry.wire, /prompt|message|tool_output/i);
  }
});

test("protocol v2 builders share exact generation and omit absent options", () => {
  const event = buildProtocolV2Event({
    event_uuid: "00000000-0000-4000-8000-000000000001",
    event: "session_start",
    normalized_host: "pi-host",
    session_id: "session-root",
    process_instance_id: "10000000-0000-4000-8000-000000000001",
    sequence: 1,
    origin_at: "2026-08-18T02:00:00.000Z",
    occurred_at: "2026-08-18T02:00:00.000Z",
    delivery_source: "live",
    reason: "startup",
    lineage: { kind: "root" },
  });
  assert.deepEqual(buildProtocolV2Envelope([event]), {
    protocol: PI_PROTOCOL_V2,
    reporter_version: PI_PROTOCOL_V2_REPORTER_VERSION,
    quill_build: PI_PROTOCOL_V2_QUILL_BUILD,
    capability_digest: PI_PROTOCOL_V2_CAPABILITY_DIGEST,
    events: [event],
  });
  assert.equal("previous_session_id" in event, false);
  assert.equal("agent_role" in event, false);
  assert.equal(
    buildQuillTrackingEntry(event).data.reporter.capability_digest,
    PI_PROTOCOL_V2_CAPABILITY_DIGEST,
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Persisted lifecycle evidence]]
test("persistent lifecycle appends the same protocol v2 event before live delivery", async () => {
  await withHome(
    {
      url: "http://127.0.0.1:19876",
      secret: "secret",
      hostname: "PI-HOST.EXAMPLE.COM",
    },
    async () => {
      const order = [];
      const pi = fakePi({ onAppend: () => order.push("append") });
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        const target = String(url);
        if (target.endsWith("/api/v1/pi/track")) order.push("fetch");
        calls.push({ url: target, body: JSON.parse(options.body) });
        return httpResponse(202);
      };
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          context("persisted-session"),
        );
        await flushRequests();

        assert.deepEqual(order.slice(0, 2), ["append", "fetch"]);
        assert.equal(pi.entries.length, 1);
        assert.equal(pi.entries[0].customType, "quill-tracking");
        const entry = pi.entries[0].data;
        const envelope = calls.find((call) =>
          call.url.endsWith("/api/v1/pi/track"),
        ).body;
        assert.equal(envelope.protocol, PI_PROTOCOL_V2);
        assert.equal(envelope.events[0].normalized_host, "pi-host");
        assert.equal(entry.schema, 2);
        assert.equal(entry.event_uuid, envelope.events[0].event_uuid);
        assert.equal(entry.event, envelope.events[0].event);
        assert.equal(
          entry.process_instance_id,
          envelope.events[0].process_instance_id,
        );
        assert.equal(entry.sequence, envelope.events[0].sequence);
        assert.equal(entry.reporter.quill_build, PI_PROTOCOL_V2_QUILL_BUILD);
        assert.doesNotMatch(
          JSON.stringify(entry),
          /prompt|message|tool_output/i,
        );
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Typed bounded delivery]]
test("persistent live sessions reannounce every 30 seconds without overlapping", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      let finishReplay;
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        if (!String(url).endsWith("/api/v1/pi/track")) {
          return httpResponse(202);
        }
        calls.push(JSON.parse(options.body));
        if (calls.length === 2) {
          return new Promise((resolve) => {
            finishReplay = () => resolve(httpResponse(202));
          });
        }
        return httpResponse(202);
      };
      const ctx = context("periodic-recovery");
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          ctx,
        );
        await flushRequests();
        assert.equal(calls.length, 1);
        assert.equal(pi.entries.length, 1);

        t.mock.timers.tick(30_000);
        await flushRequests();
        assert.equal(calls.length, 2);
        assert.deepEqual(calls[1], calls[0]);
        assert.equal(pi.entries.length, 1);

        t.mock.timers.tick(30_000);
        await flushRequests();
        assert.equal(calls.length, 2);

        finishReplay();
        await flushRequests();
        t.mock.timers.tick(30_000);
        await flushRequests();
        assert.equal(calls.length, 3);
        assert.deepEqual(calls[2], calls[0]);
        assert.equal(pi.entries.length, 1);

        await pi.handlers.get("session_shutdown")[0](
          { type: "session_shutdown", reason: "quit" },
          ctx,
        );
        assert.equal(calls.length, 4);
        assert.equal(calls[3].events[0].event, "session_end");
        t.mock.timers.tick(60_000);
        await flushRequests();
        assert.equal(calls.length, 4);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Typed bounded delivery]]
test("reporter replacement releases the prior live reannouncement", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      let requests = 0;
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url) => {
        if (String(url).endsWith("/api/v1/pi/track")) requests += 1;
        return httpResponse(202);
      };
      const ctx = context("released-recovery");
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          ctx,
        );
        await flushRequests();
        assert.equal(requests, 1);

        quill(pi.api);
        t.mock.timers.tick(60_000);
        await flushRequests();
        assert.equal(requests, 1);

        await pi.handlers.get("session_shutdown")[0](
          { type: "session_shutdown", reason: "quit" },
          ctx,
        );
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#No-session tracking boundary]]
test("no-session mode keeps root tools and router but writes and sends no tracking", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      let requests = 0;
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async () => {
        requests += 1;
        return httpResponse(202);
      };
      const ctx = context("no-session", { sessionFile: undefined });
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          ctx,
        );
        pi.handlers.get("model_select")[0](
          {
            type: "model_select",
            model: { provider: "openai", id: "gpt-5" },
            source: "set",
          },
          ctx,
        );
        pi.handlers.get("input")[0]({ type: "input", text: "private" }, ctx);
        await flushRequests();
        t.mock.timers.tick(60_000);
        await flushRequests();
        await pi.handlers.get("session_shutdown")[0](
          { type: "session_shutdown", reason: "quit" },
          ctx,
        );
        await flushRequests();

        assert.equal(pi.entries.length, 0);
        assert.equal(requests, 0);
        assert.deepEqual([...pi.tools.keys()], TOOL_NAMES);
        assert.equal(pi.handlers.get("tool_call")?.length, 1);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Typed bounded delivery]]
test("transient tracking failures retry once and auth reloads once", async () => {
  for (const responses of [
    [new Error("timeout"), httpResponse(202)],
    [
      httpResponse(429, { code: "rate_limited", retry_after_ms: 0 }),
      httpResponse(202),
    ],
    [
      httpResponse(503, { code: "unavailable", retry_after_ms: 0 }),
      httpResponse(202),
    ],
    [httpResponse(401), httpResponse(202)],
  ]) {
    await withHome(
      { url: "http://127.0.0.1:19876", secret: "secret" },
      async () => {
        const pi = fakePi();
        quill(pi.api);
        let requests = 0;
        const oldFetch = globalThis.fetch;
        globalThis.fetch = async (url) => {
          if (!String(url).endsWith("/api/v1/pi/track")) {
            return httpResponse(202);
          }
          const response = responses[requests++];
          if (response instanceof Error) throw response;
          return response;
        };
        try {
          pi.handlers.get("session_start")[0](
            { type: "session_start", reason: "startup" },
            context(`retry-${Math.random()}`),
          );
          await flushRequests();
          assert.equal(requests, 2);
        } finally {
          globalThis.fetch = oldFetch;
        }
      },
    );
  }
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Typed bounded delivery]]
test("unknown session reannounces once and protocol mismatch makes live push inert", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        if (!String(url).endsWith("/api/v1/pi/track")) {
          return httpResponse(202);
        }
        const body = JSON.parse(options.body);
        calls.push(body);
        if (calls.length === 2) {
          return httpResponse(409, {
            code: "unknown_session",
            message: "reannounce",
          });
        }
        return httpResponse(202);
      };
      const ctx = context("recover-session");
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          ctx,
        );
        await flushRequests();
        await pi.handlers.get("session_shutdown")[0](
          { type: "session_shutdown", reason: "quit" },
          ctx,
        );

        assert.equal(calls.length, 4);
        assert.equal(calls[1].events[0].event, "session_end");
        assert.equal(calls[2].events[0].event, "session_start");
        assert.equal(
          calls[3].events[0].event_uuid,
          calls[1].events[0].event_uuid,
        );
        assert.equal(pi.entries.length, 2);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );

  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      let requests = 0;
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url) => {
        if (!String(url).endsWith("/api/v1/pi/track")) {
          return httpResponse(202);
        }
        requests += 1;
        return httpResponse(426, {
          code: "protocol_mismatch",
          message: "exact pair required",
        });
      };
      const ctx = context("inert-session");
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          ctx,
        );
        await flushRequests();
        t.mock.timers.tick(60_000);
        await flushRequests();
        assert.equal(requests, 1);
        await pi.handlers.get("session_shutdown")[0](
          { type: "session_shutdown", reason: "quit" },
          ctx,
        );
        assert.equal(requests, 1);
        assert.equal(pi.entries.length, 2);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Tracking capability boundary]]
test("child mode exposes tracking without root tools or router", async () => {
  await withHome({ url: "http://127.0.0.1:19876", secret: "secret" }, () => {
    process.env.PI_SUBAGENT_CHILD = "1";
    process.env.PI_SUBAGENT_PARENT_SESSION = "parent-session";
    const pi = fakePi();
    quill(pi.api);
    assert.equal(pi.tools.size, 0);
    assert.equal(pi.handlers.has("tool_call"), false);
    assert.equal(pi.handlers.get("session_start")?.length, 1);
  });
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Self disabling load]]
test("missing, malformed, and remote config leave Pi unchanged", async () => {
  const oldWarn = console.warn;
  console.warn = () => {};
  try {
    for (const config of [
      undefined,
      "{",
      { url: "https://quill.example.test", secret: "secret" },
      { url: "http://127.0.0.1.evil.test:19876", secret: "secret" },
    ]) {
      await withHome(config, () => {
        const pi = fakePi();
        assert.doesNotThrow(() => quill(pi.api));
        assert.equal(pi.tools.size, 0);
        assert.equal(pi.handlers.size, 0);
      });
    }
  } finally {
    console.warn = oldWarn;
  }
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#No Quill inertness]]
test("invalid config performs no disk writes and emits one notice", async () => {
  const root = configRoot("{");
  const oldHome = process.env.HOME;
  const warnings = [];
  const oldWarn = console.warn;
  process.env.HOME = root;
  console.warn = (...parts) => warnings.push(parts.join(" "));
  try {
    const before = readdirSync(root, { recursive: true }).sort();
    quill(fakePi().api);
    quill(fakePi().api);
    assert.deepEqual(readdirSync(root, { recursive: true }).sort(), before);
    assert.equal(warnings.length, 1);
    assert.match(warnings[0], /Quill.*config/i);
  } finally {
    console.warn = oldWarn;
    if (oldHome === undefined) delete process.env.HOME;
    else process.env.HOME = oldHome;
    rmSync(root, { recursive: true, force: true });
  }
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Tracking registration]]
test("registers every production tracking handler", async () => {
  await withHome({ url: "http://127.0.0.1:19876", secret: "secret" }, () => {
    const pi = fakePi();
    quill(pi.api);
    for (const event of [
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
    ]) {
      assert.equal(pi.handlers.get(event)?.length, 1, event);
    }
  });
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Reporter coexistence]]
test("reporter candidates classify channels and require exact unofficial opt-in", () => {
  const home = "/home/test";
  const agentDir = `${home}/.pi/agent`;
  const cwd = `${home}/work/project`;
  const paths = {
    managed: `${agentDir}/extensions/quill.ts`,
    npm: `${agentDir}/npm/node_modules/@sharaf-nassar/quill-pi/quill.ts`,
    project: `${cwd}/.pi/extensions/quill.ts`,
    development: `${home}/work/quill/src-tauri/pi-integration/quill.ts`,
  };

  for (const [installChannel, extensionPath] of Object.entries(paths)) {
    const candidate = reporterCandidateForPath(extensionPath, {
      home,
      agentDir,
      cwd,
      selectedPath:
        installChannel === "project" || installChannel === "development"
          ? extensionPath
          : undefined,
    });
    assert.equal(candidate.install_channel, installChannel);
    assert.equal(candidate.extension_path, resolve(extensionPath));
    assert.equal(candidate.eligible, true);
  }

  for (const installChannel of ["project", "development"]) {
    assert.equal(
      reporterCandidateForPath(paths[installChannel], {
        home,
        agentDir,
        cwd,
      }).eligible,
      false,
    );
  }
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Reporter coexistence]]
test("reporter election rejects both skew directions and follows channel precedence", () => {
  const candidate = (install_channel, overrides = {}) => ({
    install_channel,
    extension_path: `/${install_channel}/quill.ts`,
    eligible: true,
    protocol: PI_PROTOCOL_V2,
    reporter_version: PI_PROTOCOL_V2_REPORTER_VERSION,
    quill_build: PI_PROTOCOL_V2_QUILL_BUILD,
    capability_digest: PI_PROTOCOL_V2_CAPABILITY_DIGEST,
    capabilities: PI_PROTOCOL_V2_CAPABILITIES,
    ...overrides,
  });
  const project = candidate("project");
  const npm = candidate("npm");
  const managed = candidate("managed");

  assert.equal(
    electReporterCandidate([project, npm, managed], REPORTER_CONTRACT),
    managed,
  );
  assert.equal(electReporterCandidate([project, npm], REPORTER_CONTRACT), npm);
  assert.equal(electReporterCandidate([project], REPORTER_CONTRACT), project);
  assert.equal(
    electReporterCandidate(
      [candidate("managed", { protocol: PI_PROTOCOL_V2 - 1 })],
      REPORTER_CONTRACT,
    ),
    undefined,
  );
  assert.equal(
    electReporterCandidate(
      [candidate("managed", { protocol: PI_PROTOCOL_V2 + 1 })],
      REPORTER_CONTRACT,
    ),
    undefined,
  );
  assert.equal(
    electReporterCandidate(
      [candidate("managed", { eligible: false }), project],
      REPORTER_CONTRACT,
    ),
    project,
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Reporter coexistence]]
test("coexisting copies register and emit each stable event once", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret", hostname: "pi-host" },
    async () => {
      const pi = fakePi();
      const laterCopy = fakePi();
      quill(pi.api);
      quill(laterCopy.api);
      assert.equal(laterCopy.handlers.size, 0);
      assert.equal(laterCopy.tools.size, 0);

      for (const event of [
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
      ]) {
        assert.equal(pi.handlers.get(event)?.length, 1, event);
      }

      const calls = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        calls.push({ url: String(url), body: JSON.parse(options.body) });
        return {
          ok: true,
          status: 202,
          json: async () => ({}),
          body: { cancel: async () => {} },
        };
      };
      const ctx = context("coexistence-session");
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          ctx,
        );
        await flushRequests();
        pi.handlers.get("message_end")[0](
          {
            type: "message_end",
            message: {
              role: "assistant",
              provider: "openai",
              model: "gpt-5",
              responseId: "coexistence-response",
              timestamp: 1_765_699_202_000,
              content: [{ type: "text", text: "not sent" }],
              usage: {
                input: 2,
                output: 3,
                cacheRead: 0,
                cacheWrite: 0,
                totalTokens: 5,
                cost: {
                  input: 0,
                  output: 0,
                  cacheRead: 0,
                  cacheWrite: 0,
                  total: 0,
                },
              },
            },
          },
          ctx,
        );
        await flushRequests();
        await pi.handlers.get("session_shutdown")[0](
          { type: "session_shutdown", reason: "quit" },
          ctx,
        );

        const events = calls
          .filter((call) => call.url.endsWith("/api/v1/pi/track"))
          .flatMap((call) => call.body.events);
        for (const type of ["session_start", "model", "usage", "session_end"]) {
          assert.equal(
            events.filter((event) => (event.event || event.type) === type)
              .length,
            1,
            type,
          );
        }
        assert.equal(
          new Set(events.map((event) => event.event_uuid)).size,
          events.length,
        );
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Feature gates]]
test("disabled and mismatched reporter contracts register nothing", async () => {
  for (const pi_reporter of [
    { ...REPORTER_CONTRACT, enabled: false },
    { ...REPORTER_CONTRACT, reporter_version: "99.0.0" },
    { ...REPORTER_CONTRACT, quill_build: "99.0.0" },
  ]) {
    await withHome(
      { url: "http://127.0.0.1:19876", secret: "secret", pi_reporter },
      () => {
        const pi = fakePi();
        quill(pi.api);
        assert.equal(pi.tools.size, 0);
        assert.equal(pi.handlers.size, 0);
      },
    );
  }
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Reporter coexistence]]
test("a pre-broker claim stays active and emits one reload remediation", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    (root) => {
      const claimsKey = Symbol.for("quill.pi.reporter.claims");
      const quillRoot = join(root, ".config", "quill");
      const claims = globalThis[claimsKey] || new Set();
      globalThis[claimsKey] = claims;
      claims.add(quillRoot);
      const warnings = [];
      const oldWarn = console.warn;
      console.warn = (...parts) => warnings.push(parts.join(" "));
      try {
        const pi = fakePi();
        quill(pi.api);
        quill(pi.api);
        assert.equal(pi.handlers.size, 0);
        assert.equal(warnings.length, 1);
        assert.match(warnings[0], /ReporterReloadRequired/);
        assert.match(warnings[0], /reload/i);
      } finally {
        console.warn = oldWarn;
        claims.delete(quillRoot);
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Tracking envelopes]]
test("tracking handlers emit versioned lifecycle, usage, and runtime envelopes", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret", hostname: "PI-HOST" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        calls.push({ url: String(url), body: JSON.parse(options.body) });
        return {
          ok: true,
          status: 202,
          json: async () => ({}),
          body: { cancel: async () => {} },
        };
      };
      const ctx = context("session-track", {
        model: { provider: "anthropic", id: "claude-sonnet" },
      });
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          ctx,
        );
        pi.handlers.get("agent_start")[0]({ type: "agent_start" }, ctx);
        pi.handlers.get("agent_settled")[0]({ type: "agent_settled" }, ctx);
        pi.handlers.get("input")[0](
          { type: "input", text: "secret prompt", source: "interactive" },
          ctx,
        );
        pi.handlers.get("turn_start")[0](
          { type: "turn_start", turnIndex: 0, timestamp: 1_765_699_201_000 },
          ctx,
        );
        pi.handlers.get("tool_execution_start")[0](
          {
            type: "tool_execution_start",
            toolCallId: "tool-1",
            toolName: "read",
            args: { path: "/secret" },
          },
          ctx,
        );
        pi.handlers.get("tool_execution_end")[0](
          {
            type: "tool_execution_end",
            toolCallId: "tool-1",
            toolName: "read",
            result: "secret",
            isError: false,
          },
          ctx,
        );
        pi.handlers.get("model_select")[0](
          {
            type: "model_select",
            model: { provider: "openai", id: "gpt-5" },
            source: "set",
          },
          ctx,
        );
        const assistant = {
          role: "assistant",
          provider: "openai",
          model: "gpt-5",
          responseId: "response-1",
          timestamp: 1_765_699_202_000,
          content: [{ type: "text", text: "secret answer" }],
          usage: {
            input: 11,
            output: 7,
            cacheRead: 3,
            cacheWrite: 2,
            totalTokens: 23,
            cost: {
              input: 0.1,
              output: 0.2,
              cacheRead: 0.03,
              cacheWrite: 0.02,
              total: 0.35,
            },
          },
        };
        pi.handlers.get("message_end")[0](
          { type: "message_end", message: assistant },
          ctx,
        );
        const nextAssistant = {
          ...assistant,
          responseId: "response-2",
          timestamp: 1_765_699_203_000,
          usage: {
            input: 5,
            output: 4,
            cacheRead: 1,
            cacheWrite: 6,
            totalTokens: 16,
            cost: {
              input: 0.05,
              output: 0.08,
              cacheRead: 0.01,
              cacheWrite: 0.06,
              total: 0.2,
            },
          },
        };
        pi.handlers.get("message_end")[0](
          { type: "message_end", message: nextAssistant },
          ctx,
        );
        pi.handlers.get("message_end")[0](
          { type: "message_end", message: nextAssistant },
          ctx,
        );
        pi.handlers.get("turn_end")[0](
          {
            type: "turn_end",
            turnIndex: 0,
            message: assistant,
            toolResults: [],
          },
          ctx,
        );
        await pi.handlers.get("session_shutdown")[0](
          { type: "session_shutdown", reason: "quit" },
          ctx,
        );
        await flushRequests();

        const track = calls.filter((call) =>
          call.url.endsWith("/api/v1/pi/track"),
        );
        assert.ok(track.length >= 8);
        const lifecycle = track.filter(({ body }) => body.protocol === 2);
        const native = track.filter(({ body }) => body.protocol === 1);
        assert.equal(lifecycle.length, 2);
        assert.ok(
          lifecycle.every(
            ({ body }) =>
              body.reporter_version === PI_PROTOCOL_V2_REPORTER_VERSION &&
              body.quill_build === PI_PROTOCOL_V2_QUILL_BUILD,
          ),
        );
        assert.ok(
          native.every(
            ({ body }) =>
              typeof body.extension_version === "string" &&
              body.min_quill_version,
          ),
        );
        const events = track.flatMap(({ body }) => body.events);
        assert.ok(
          events.some(
            (event) =>
              event.event === "session_start" && event.lineage.kind === "root",
          ),
        );
        assert.ok(
          events.some(
            (event) => event.event === "session_end" && event.reason === "quit",
          ),
        );
        assert.ok(
          events.some(
            (event) => event.type === "model" && event.model === "gpt-5",
          ),
        );
        const usageBatches = track.filter(({ body }) =>
          body.events.some(
            (event) =>
              event.event_uuid === "pi_e56c5118f3692e9dabf155823c5a9cbb",
          ),
        );
        assert.equal(usageBatches.length, 2);
        assert.deepEqual(usageBatches[0].body, {
          protocol: 1,
          extension_version: "0.2.0",
          min_quill_version: "0.9.0",
          events: [
            {
              type: "model",
              event_uuid: "pi_b89631f1c47149577bcff18311333a55",
              session_id: "session-track",
              hostname: "PI-HOST",
              timestamp: "2025-12-14T08:00:03.000Z",
              model_provider: "openai",
              model: "gpt-5",
            },
            {
              type: "usage",
              event_uuid: "pi_e56c5118f3692e9dabf155823c5a9cbb",
              session_id: "session-track",
              hostname: "PI-HOST",
              timestamp: "2025-12-14T08:00:03.000Z",
              model_provider: "openai",
              model: "gpt-5",
              input_tokens: 5,
              output_tokens: 4,
              cache_read_tokens: 1,
              cache_write_tokens: 6,
              cost: {
                input: 0.05,
                output: 0.08,
                cache_read: 0.01,
                cache_write: 0.06,
                total: 0.2,
              },
            },
          ],
        });
        assert.equal(
          usageBatches[1].body.events[1].event_uuid,
          usageBatches[0].body.events[1].event_uuid,
        );

        const messages = calls
          .filter((call) => call.url.endsWith("/api/v1/sessions/messages"))
          .flatMap((call) => call.body.messages);
        assert.ok(
          messages.some((message) => message.event_kinds.includes("user_text")),
        );
        assert.ok(
          messages.some((message) => message.event_kinds.includes("asst_text")),
        );
        assert.ok(
          messages.some((message) =>
            message.event_kinds.includes("asst_tool_use"),
          ),
        );
        assert.ok(
          messages.some((message) =>
            message.event_kinds.includes("user_tool_result"),
          ),
        );
        assert.ok(messages.every((message) => message.content === ""));
        assert.equal(
          new Set(messages.map((message) => message.uuid)).size,
          messages.length,
        );
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Tracking envelopes]]
test("assistant text runtime follows text messages, not turn completion", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const messages = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        if (String(url).endsWith("/api/v1/sessions/messages")) {
          messages.push(...JSON.parse(options.body).messages);
        }
        return httpResponse(202);
      };
      try {
        const ctx = context("text-semantics");
        pi.handlers.get("turn_end")[0]({ type: "turn_end", turnIndex: 0 }, ctx);
        pi.handlers.get("message_end")[0](
          {
            type: "message_end",
            message: {
              role: "assistant",
              provider: "openai",
              model: "gpt-5",
              responseId: "tool-only",
              timestamp: 1_765_699_202_000,
              content: [{ type: "toolCall", name: "read" }],
              usage: { input: 1, output: 1 },
            },
          },
          ctx,
        );
        pi.handlers.get("message_end")[0](
          {
            type: "message_end",
            message: {
              role: "assistant",
              provider: "openai",
              model: "gpt-5",
              responseId: "with-text",
              timestamp: 1_765_699_203_000,
              content: [{ type: "text", text: "done" }],
              usage: { input: 1, output: 1 },
            },
          },
          ctx,
        );
        await flushRequests();
        assert.equal(
          messages.filter((message) => message.event_kinds.includes("asst_text"))
            .length,
          1,
        );
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Handshake and lineage]]
test("session start resolves lineage once and notifies only persisted sessions", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const parent = join(process.env.HOME, "parent.jsonl");
      const child = join(process.env.HOME, "child.jsonl");
      writeFileSync(
        parent,
        `${JSON.stringify({ type: "session", id: "parent-id" })}\nignored\n`,
      );
      writeFileSync(
        child,
        `${JSON.stringify({ type: "session", id: "child-id" })}\n`,
      );
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        calls.push({ url: String(url), body: JSON.parse(options.body) });
        return { ok: true, status: 202, body: { cancel: async () => {} } };
      };
      try {
        const ctx = context("child-id", {
          sessionFile: child,
          parentSession: parent,
        });
        pi.handlers.get("session_start")[0](
          {
            type: "session_start",
            reason: "fork",
            previousSessionFile: parent,
          },
          ctx,
        );
        await flushRequests();
        const start = calls.find((call) =>
          call.url.endsWith("/api/v1/pi/track"),
        ).body.events[0];
        assert.deepEqual(start.lineage, {
          kind: "linked",
          parent_session_id: "parent-id",
        });
        assert.equal(start.previous_session_id, "parent-id");
        const notify = calls.find((call) =>
          call.url.endsWith("/api/v1/sessions/notify"),
        );
        assert.deepEqual(notify.body.lineage, {
          kind: "linked",
          parent_session_id: "parent-id",
        });
        assert.equal(
          calls.filter((call) => call.url.endsWith("/api/v1/sessions/notify"))
            .length,
          1,
        );

        pi.handlers.get("session_start")[0](
          {
            type: "session_start",
            reason: "resume",
            previousSessionFile: parent,
          },
          ctx,
        );
        await flushRequests();
        assert.deepEqual(
          calls
            .filter((call) => call.url.endsWith("/api/v1/sessions/notify"))
            .at(-1).body.lineage,
          { kind: "linked", parent_session_id: "parent-id" },
        );

        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          context("ephemeral"),
        );
        await flushRequests();
        assert.equal(
          calls.filter((call) => call.url.endsWith("/api/v1/sessions/notify"))
            .length,
          2,
        );
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Environment Agent Lineage]]
test("env-marked Pi child reports agent lineage without a parent header", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const child = join(process.env.HOME, "child.jsonl");
      writeFileSync(
        child,
        `${JSON.stringify({ type: "session", id: "child-id" })}\n`,
      );
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      const oldChild = process.env.PI_SUBAGENT_CHILD;
      const oldParent = process.env.PI_SUBAGENT_PARENT_SESSION;
      process.env.PI_SUBAGENT_CHILD = "1";
      process.env.PI_SUBAGENT_PARENT_SESSION = "parent-id";
      globalThis.fetch = async (url, options) => {
        calls.push({ url: String(url), body: JSON.parse(options.body) });
        return { ok: true, status: 202, body: { cancel: async () => {} } };
      };
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          context("child-id", { sessionFile: child }),
        );
        await flushRequests();

        const lineage = { kind: "agent", parent_session_id: "parent-id" };
        assert.deepEqual(
          calls.find((call) => call.url.endsWith("/api/v1/pi/track")).body
            .events[0].lineage,
          lineage,
        );
        assert.deepEqual(
          calls.find((call) => call.url.endsWith("/api/v1/sessions/notify"))
            .body.lineage,
          lineage,
        );
      } finally {
        globalThis.fetch = oldFetch;
        if (oldChild === undefined) delete process.env.PI_SUBAGENT_CHILD;
        else process.env.PI_SUBAGENT_CHILD = oldChild;
        if (oldParent === undefined)
          delete process.env.PI_SUBAGENT_PARENT_SESSION;
        else process.env.PI_SUBAGENT_PARENT_SESSION = oldParent;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Invalid Environment Agent Lineage]]
test("env-marked Pi child with an invalid parent reports unresolved lineage", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      const oldChild = process.env.PI_SUBAGENT_CHILD;
      const oldParent = process.env.PI_SUBAGENT_PARENT_SESSION;
      process.env.PI_SUBAGENT_CHILD = "1";
      process.env.PI_SUBAGENT_PARENT_SESSION = " parent-id ";
      globalThis.fetch = async (url, options) => {
        calls.push({ url: String(url), body: JSON.parse(options.body) });
        return { ok: true, status: 202, body: { cancel: async () => {} } };
      };
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          context("child-id"),
        );
        await flushRequests();

        assert.deepEqual(
          calls.find((call) => call.url.endsWith("/api/v1/pi/track")).body
            .events[0].lineage,
          { kind: "unresolved", reason: "subagent_parent_unavailable" },
        );
      } finally {
        globalThis.fetch = oldFetch;
        if (oldChild === undefined) delete process.env.PI_SUBAGENT_CHILD;
        else process.env.PI_SUBAGENT_CHILD = oldChild;
        if (oldParent === undefined)
          delete process.env.PI_SUBAGENT_PARENT_SESSION;
        else process.env.PI_SUBAGENT_PARENT_SESSION = oldParent;
      }
    },
  );
});

// Break: trusting a header-derived parent id lets a marked child parent itself.
// @lat: [[pi-extension-tests#Pi Extension Test Specs#Invalid Header Agent Lineage]]
test("env-marked Pi child rejects its own header-derived parent id", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const parent = join(process.env.HOME, "parent.jsonl");
      writeFileSync(
        parent,
        `${JSON.stringify({ type: "session", id: "child-id" })}\n`,
      );
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      const oldChild = process.env.PI_SUBAGENT_CHILD;
      const oldParent = process.env.PI_SUBAGENT_PARENT_SESSION;
      process.env.PI_SUBAGENT_CHILD = "1";
      delete process.env.PI_SUBAGENT_PARENT_SESSION;
      globalThis.fetch = async (url, options) => {
        calls.push({ url: String(url), body: JSON.parse(options.body) });
        return { ok: true, status: 202, body: { cancel: async () => {} } };
      };
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          context("child-id", { parentSession: parent }),
        );
        await flushRequests();

        assert.deepEqual(
          calls.find((call) => call.url.endsWith("/api/v1/pi/track")).body
            .events[0].lineage,
          { kind: "unresolved", reason: "subagent_parent_unavailable" },
        );
      } finally {
        globalThis.fetch = oldFetch;
        if (oldChild === undefined) delete process.env.PI_SUBAGENT_CHILD;
        else process.env.PI_SUBAGENT_CHILD = oldChild;
        if (oldParent === undefined)
          delete process.env.PI_SUBAGENT_PARENT_SESSION;
        else process.env.PI_SUBAGENT_PARENT_SESSION = oldParent;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Turn-end search freshness]]
test("turn end repeats the start notify with unchanged lineage", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const parent = join(process.env.HOME, "parent.jsonl");
      const child = join(process.env.HOME, "child.jsonl");
      writeFileSync(
        parent,
        `${JSON.stringify({ type: "session", id: "parent-id" })}\n`,
      );
      writeFileSync(
        child,
        `${JSON.stringify({ type: "session", id: "child-id" })}\n`,
      );
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        calls.push({ url: String(url), body: JSON.parse(options.body) });
        return { ok: true, status: 202, body: { cancel: async () => {} } };
      };
      try {
        const ctx = context("child-id", {
          sessionFile: child,
          parentSession: parent,
        });
        pi.handlers.get("session_start")[0](
          {
            type: "session_start",
            reason: "fork",
            previousSessionFile: parent,
          },
          ctx,
        );
        await flushRequests();
        writeFileSync(parent, "malformed");
        pi.handlers.get("turn_end")[0]({ type: "turn_end", turnIndex: 0 }, ctx);
        await flushRequests();

        const notifies = calls.filter((call) =>
          call.url.endsWith("/api/v1/sessions/notify"),
        );
        assert.equal(notifies.length, 2);
        assert.deepEqual(notifies[1].body, notifies[0].body);
        assert.deepEqual(notifies[1].body.lineage, {
          kind: "linked",
          parent_session_id: "parent-id",
        });
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Deferred transcript notify]]
test("a named but unwritten transcript defers notify to turn end", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      // Pi names the session file at start and only writes it once the first
      // assistant message lands, so the path exists before the file does.
      const child = join(process.env.HOME, "unwritten.jsonl");
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        calls.push({ url: String(url), body: JSON.parse(options.body) });
        return { ok: true, status: 202, body: { cancel: async () => {} } };
      };
      const notifies = () =>
        calls.filter((call) => call.url.endsWith("/api/v1/sessions/notify"));
      try {
        const ctx = context("child-id", { sessionFile: child });
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          ctx,
        );
        await flushRequests();

        assert.equal(notifies().length, 0);
        const start = calls.find((call) =>
          call.url.endsWith("/api/v1/pi/track"),
        ).body.events[0];
        assert.equal(start.event, "session_start");
        assert.equal("ephemeral" in start, false);

        writeFileSync(
          child,
          `${JSON.stringify({ type: "session", id: "child-id" })}\n`,
        );
        pi.handlers.get("turn_end")[0]({ type: "turn_end", turnIndex: 0 }, ctx);
        await flushRequests();

        assert.equal(notifies().length, 1);
        assert.equal(notifies()[0].body.jsonl_path, child);
        assert.deepEqual(notifies()[0].body.lineage, { kind: "root" });
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Process lifecycle identity]]
test("process identity persists and lifecycle sequence advances across reload", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const entries = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async () => httpResponse(202);
      try {
        for (let attempt = 0; attempt < 2; attempt += 1) {
          const pi = fakePi({ onAppend: (_type, data) => entries.push(data) });
          quill(pi.api);
          pi.handlers.get("session_start")[0](
            { type: "session_start", reason: "reload" },
            context("stable-session"),
          );
          await flushRequests();
          await pi.handlers.get("session_shutdown")[0](
            { type: "session_shutdown", reason: "reload" },
            context("stable-session"),
          );
        }
        assert.equal(entries.length, 4);
        assert.equal(
          new Set(entries.map((entry) => entry.process_instance_id)).size,
          1,
        );
        assert.deepEqual(
          entries.map((entry) => entry.sequence),
          [1, 2, 3, 4],
        );
        assert.equal(new Set(entries.map((entry) => entry.event_uuid)).size, 4);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Persisted source durability]]
test("failed live delivery creates no spool or extension log", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const oldFetch = globalThis.fetch;
      const oldError = console.error;
      globalThis.fetch = () => Promise.reject(new Error("connection refused"));
      console.error = () => {};
      try {
        pi.handlers.get("session_start")[0](
          { type: "session_start", reason: "startup" },
          context("persisted-offline"),
        );
        await flushRequests();
        assert.equal(pi.entries.length, 1);
        const artifacts = readdirSync(
          join(process.env.HOME, ".config", "quill"),
        );
        assert.equal(artifacts.includes("pi-spool"), false);
        assert.equal(artifacts.includes("pi-extension.log"), false);
      } finally {
        globalThis.fetch = oldFetch;
        console.error = oldError;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Tool registration boundary]]
test("registers the prefixed tools with plain JSON Schema", async () => {
  await withHome(
    {
      url: "http://localhost:19876",
      context_url: "http://localhost:19877",
      secret: "secret",
    },
    () => {
      const pi = fakePi();
      quill(pi.api);
      assert.deepEqual([...pi.tools.keys()], TOOL_NAMES);
      for (const tool of pi.tools.values()) {
        assert.equal(Object.getPrototypeOf(tool.parameters), Object.prototype);
        assert.equal(tool.parameters.type, "object");
        assert.equal(typeof tool.execute, "function");
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Feature gates]]
test("rendered feature flags independently gate tools and telemetry", async () => {
  await withHome(
    { url: "http://localhost:19876", secret: "secret" },
    async () => {
      const source = readFileSync(
        join(import.meta.dirname, "quill.ts"),
        "utf8",
      );
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async () => ({
        ok: true,
        status: 202,
        body: { cancel: async () => {} },
      });
      try {
        for (const [flags, expectedTools, requiredHandlers] of [
          [
            {
              context_preservation: false,
              activity_tracking: true,
              context_telemetry: true,
            },
            0,
            [
              "session_start",
              "session_shutdown",
              "message_end",
              "tool_execution_start",
            ],
          ],
          [
            {
              context_preservation: true,
              activity_tracking: false,
              context_telemetry: false,
            },
            8,
            ["tool_call", "session_shutdown"],
          ],
        ]) {
          const path = join(
            process.env.HOME,
            `rendered-${flags.context_preservation}-${flags.activity_tracking}.mjs`,
          );
          writeFileSync(path, renderFeatures(source, flags));
          const rendered = (await import(pathToFileURL(path).href)).default;
          process.env.QUILL_PI_REPORTER_PATH = path;
          const pi = fakePi();
          rendered(pi.api);
          assert.equal(pi.tools.size, expectedTools);
          assert.deepEqual(
            [...pi.handlers.keys()]
              .filter((name) => requiredHandlers.includes(name))
              .sort(),
            requiredHandlers.toSorted(),
          );
          assert.equal(
            pi.handlers.has("session_start"),
            flags.activity_tracking,
          );
          await pi.handlers.get("session_shutdown")[0](
            { type: "session_shutdown", reason: "reload" },
            context(),
          );
        }
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Exception containment]]
test("registration and handler exceptions never escape", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const rejecting = fakePi({ registerError: new Error("duplicate") });
      assert.doesNotThrow(() => quill(rejecting.api));
      assert.equal(rejecting.registrationAttempts(), TOOL_NAMES.length);
      await rejecting.handlers.get("session_shutdown")[0](
        { type: "session_shutdown", reason: "reload" },
        context("registration-reset", { sessionFile: undefined }),
      );

      const pi = fakePi();
      quill(pi.api);
      const oldFetch = globalThis.fetch;
      globalThis.fetch = () => {
        throw new Error("down");
      };
      try {
        for (const tool of pi.tools.values()) {
          const result = await tool.execute(
            "call",
            {},
            undefined,
            undefined,
            context(),
          );
          assert.equal(result.isError, true);
          assert.equal(result.details.ok, false);
          assert.equal(result.details.error.type, "quill_unavailable");
          assert.ok(result.content[0].text.length < 80);
        }
        for (const handler of allHandlers(pi)) {
          assert.doesNotThrow(() =>
            handler(
              {},
              {
                cwd: "/tmp",
                sessionManager: {
                  getSessionId: () => {
                    throw new Error("bad session");
                  },
                },
              },
            ),
          );
        }
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#HTTP tool contract]]
test("tools call the local session and context APIs with typed results", async () => {
  await withHome(
    {
      url: "http://[::1]:19876",
      context_url: "http://[::1]:19877",
      secret: "secret",
    },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const calls = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        calls.push({ url: String(url), options });
        return {
          ok: true,
          json: async () => ({ ok: true }),
        };
      };
      try {
        const history = await pi.tools
          .get("quill_search_history")
          .execute(
            "history",
            { query: "needle", project: "/tmp/project", limit: 3 },
            undefined,
            undefined,
            context(),
          );
        const indexed = await pi.tools
          .get("quill_index_context")
          .execute(
            "index",
            { content: "large output" },
            undefined,
            undefined,
            context(),
          );
        assert.equal(history.isError, undefined);
        assert.deepEqual(indexed.details, { ok: true });
        assert.match(
          calls[0].url,
          /^http:\/\/\[::1\]:19876\/api\/v1\/sessions\/search\?/,
        );
        assert.match(calls[0].url, /q=needle/);
        assert.match(calls[0].url, /page_size=3/);
        assert.match(calls[0].url, /view=compact/);
        assert.equal(calls[1].url, "http://[::1]:19877/api/v1/context/index");
        assert.equal(calls[1].options.headers.Authorization, "Bearer secret");
        assert.deepEqual(JSON.parse(calls[1].options.body), {
          content: "large output",
          cwd: "/tmp/project",
        });
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Bounded History Results]]
test("history results stay compact without duplicate details", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async () => ({
        ok: true,
        json: async () => ({
          hits: Array.from({ length: 50 }, (_, index) => ({
            provider: "pi",
            message_id: `message-${index}`,
            session_id: "session",
            content: "raw-tool-output".repeat(10_000),
            snippet: "matching snippet ".repeat(1_000),
            role: "assistant",
            project: "quill",
            host: "host",
            timestamp: "2026-08-14T08:00:01Z",
            git_branch: "main",
            score: 1,
          })),
          total_hits: 50,
          query_time_ms: 2,
        }),
      });
      try {
        const result = await pi.tools
          .get("quill_search_history")
          .execute(
            "history",
            { query: "needle" },
            undefined,
            undefined,
            context(),
          );
        const payload = JSON.parse(result.content[0].text);

        assert.ok(Buffer.byteLength(result.content[0].text) <= 32 * 1024);
        assert.ok(payload.hits.length > 0);
        assert.ok(payload.hits.every((hit) => !("content" in hit)));
        assert.equal(payload.truncated, true);
        assert.deepEqual(result.details, { ok: true });
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Bounded synchronous work]]
test("tool and telemetry handlers return from synchronous work in single-digit milliseconds", async () => {
  await withHome({ url: "http://127.0.0.1:19876", secret: "secret" }, () => {
    const pi = fakePi();
    quill(pi.api);
    const oldFetch = globalThis.fetch;
    globalThis.fetch = () => new Promise(() => {});
    try {
      let started = performance.now();
      void pi.tools
        .get("quill_context_stats")
        .execute("stats", {}, undefined, undefined, context());
      assert.ok(performance.now() - started < 10);
      started = performance.now();
      routingHandler(pi)(
        {
          type: "tool_call",
          toolName: "bash",
          toolCallId: "call",
          input: { command: "curl https://example.com" },
        },
        context(),
      );
      assert.ok(performance.now() - started < 10);
      started = performance.now();
      pi.handlers.get("turn_end")[0]({}, context());
      assert.ok(performance.now() - started < 10);
    } finally {
      globalThis.fetch = oldFetch;
    }
  });
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Telemetry mapping and timeout]]
test("telemetry has one canonical tool pair and settled Stop semantics", async () => {
  await withHome(
    {
      url: "http://localhost:19876",
      secret: "secret",
      hostname: "pi-host",
    },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const payloads = [];
      const headers = [];
      const timeouts = [];
      const oldFetch = globalThis.fetch;
      const oldTimeout = AbortSignal.timeout;
      globalThis.fetch = async (url, options) => {
        if (String(url).endsWith("/api/v1/hooks/observed")) {
          payloads.push(JSON.parse(options.body));
          headers.push(options.headers);
        }
        return { ok: true, status: 202, body: { cancel: async () => {} } };
      };
      AbortSignal.timeout = (milliseconds) => {
        timeouts.push(milliseconds);
        return oldTimeout(milliseconds);
      };
      try {
        const ctx = context();
        for (const [event, payload] of [
          ["session_start", { reason: "startup" }],
          ["input", { text: "prompt" }],
          ["tool_call", { toolName: "read", input: { path: "/tmp/a" } }],
          ["tool_execution_start", { toolName: "read", toolCallId: "call" }],
          ["tool_execution_end", { toolName: "read", toolCallId: "call" }],
          ["turn_start", { turnIndex: 0 }],
          ["turn_end", { turnIndex: 0 }],
          ["agent_start", {}],
          ["agent_settled", {}],
          ["session_before_compact", {}],
          ["session_compact", {}],
        ]) {
          pi.handlers.get(event).at(-1)({ type: event, ...payload }, ctx);
        }
        await flushRequests();
        assert.deepEqual(
          payloads.map((payload) => payload.hook_event).sort(),
          [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Stop",
            "PreCompact",
            "PostCompact",
          ].sort(),
        );
        assert.equal(
          payloads.filter(({ hook_event }) => hook_event === "PreToolUse").length,
          1,
        );
        assert.equal(
          payloads.filter(({ hook_event }) => hook_event === "PostToolUse").length,
          1,
        );
        assert.ok(payloads.every((payload) => payload.hook_event));
        assert.ok(payloads.every((payload) => payload.provider === "pi"));
        assert.ok(payloads.every((payload) => payload.session_id === "pi-session"));
        assert.ok(payloads.every((payload) => payload.hostname === "pi-host"));
        assert.ok(headers.every((value) => value["X-Quill-Pi-Process"]));
        assert.ok(headers.every((value) => value["X-Quill-Pi-Channel"]));

        const lib = readFileSync(
          join(
            import.meta.dirname,
            "..",
            "codex-integration",
            "scripts",
            "lib.cjs",
          ),
          "utf8",
        );
        const sharedTimeout = Number(
          lib.match(/const LOCAL_TIMEOUT_MS = (\d+);/)[1],
        );
        assert.ok(timeouts.length >= payloads.length);
        assert.ok(timeouts.every((timeout) => timeout === sharedTimeout));
      } finally {
        globalThis.fetch = oldFetch;
        AbortSignal.timeout = oldTimeout;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Telemetry mapping and timeout]]
test("configured child lineage exclusively owns subagent hook semantics", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      process.env.PI_SUBAGENT_CHILD = "1";
      process.env.PI_SUBAGENT_PARENT_SESSION = "parent-session";
      const pi = fakePi();
      quill(pi.api);
      const hooks = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        if (String(url).endsWith("/api/v1/hooks/observed")) {
          hooks.push(JSON.parse(options.body).hook_event);
        }
        return httpResponse(202);
      };
      try {
        const ctx = context("child-session");
        pi.handlers.get("agent_start")[0]({ type: "agent_start" }, ctx);
        pi.handlers.get("agent_settled")[0]({ type: "agent_settled" }, ctx);
        await flushRequests();
        assert.deepEqual(hooks, ["SubagentStart", "SubagentStop"]);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Typed bounded delivery]]
test("hook and routing telemetry surface typed non-2xx responses", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const errors = [];
      const oldFetch = globalThis.fetch;
      const oldError = console.error;
      globalThis.fetch = async (url) =>
        String(url).endsWith("/api/v1/hooks/observed")
          ? httpResponse(429, { code: "rate_limited", message: "slow down" })
          : httpResponse(503, { error: "unavailable" });
      console.error = (...parts) => errors.push(parts.join(" "));
      try {
        pi.handlers.get("tool_execution_start")[0](
          { type: "tool_execution_start", toolName: "read", toolCallId: "call" },
          context(),
        );
        const denied = await routingHandler(pi)(
          {
            type: "tool_call",
            toolName: "bash",
            toolCallId: "route",
            input: { command: "curl https://example.com" },
          },
          context(),
        );
        assert.equal(denied.block, true);
        await flushRequests();
        assert.ok(errors.some((error) => /rate_limited|slow down/.test(error)));
        assert.ok(errors.some((error) => /unavailable/.test(error)));
      } finally {
        globalThis.fetch = oldFetch;
        console.error = oldError;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Context router parity]]
test("context routing matches canonical Pi-applicable policy cases", async (t) => {
  const oldFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ body: { cancel: async () => {} } });
  t.after(() => {
    globalThis.fetch = oldFetch;
  });
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const route = routingHandler(pi);
      assert.equal(typeof route, "function");

      const call = (toolName, input, sessionId = `pi-${Math.random()}`) =>
        route(
          { type: "tool_call", toolName, toolCallId: "call", input },
          context(sessionId),
        );
      const deny = async (toolName, input, expected) => {
        const result = await call(toolName, input);
        assert.equal(
          result?.block,
          true,
          `${toolName} ${JSON.stringify(input)} should be denied`,
        );
        assert.ok(
          result.reason?.includes(expected),
          `deny reason missing ${expected}: ${result.reason}`,
        );
      };

      await deny(
        "web_fetch",
        { url: "https://example.com" },
        "quill_fetch_and_index",
      );
      for (const command of [
        "curl -sS https://example.com/api | jq .",
        "wget -q -O - https://example.org/data.json",
        `node -e 'fetch("https://api.example.com/v1/x").then(r => r.json())'`,
        `python -c 'import requests; requests.get("https://api.example.com/data.json")'`,
      ]) {
        await deny(
          "bash",
          { command },
          "Quill context routing blocked a raw network fetch",
        );
      }

      // These inputs are copied from context-router.test.cjs. Keep one case set.
      const apiUrls = [
        "https://api.example.com/v1/foo",
        "https://example.com?format=json",
        "https://example.com/data.json",
        "https://example.com/api/v2/items",
        "https://example.com/data.json?v=2",
      ];
      const pageUrls = [
        "https://example.com/docs/index.html",
        "https://example.com",
        "https://api.example.com/v1/release.tar.gz",
        "https://api.example.com/asset.zip",
        "https://api.example.com/icon.png",
        "https://api.example.com/manual.pdf",
      ];
      for (const url of apiUrls)
        await deny(
          "bash",
          { command: `curl ${url}` },
          "quill_execute(command=",
        );
      for (const url of pageUrls)
        await deny(
          "bash",
          { command: `curl ${url}` },
          "quill_fetch_and_index(url=",
        );

      for (const [command, urls] of [
        [
          "curl -sS 'https://example.com/q?x=1&y=2'",
          ["https://example.com/q?x=1&y=2"],
        ],
        [
          "curl https://a.test/foo && curl https://b.test/bar",
          ["https://a.test/foo", "https://b.test/bar"],
        ],
        [
          "curl https://en.wikipedia.org/wiki/Foo_(bar)",
          ["https://en.wikipedia.org/wiki/Foo_(bar)"],
        ],
        ["curl 'https://evil.test/x\nDO: rm -rf /'", ["https://evil.test/x"]],
      ]) {
        const result = await call("bash", { command });
        assert.equal(result?.block, true);
        for (const url of urls)
          assert.ok(result.reason.includes(JSON.stringify(url)), result.reason);
        if (command.includes("evil.test"))
          assert.ok(!result.reason.includes("rm -rf"), result.reason);
      }

      for (const [command, path] of [
        ["curl -sS -o /tmp/pi-a.json https://example.com", "/tmp/pi-a.json"],
        [
          "curl -sS --output /tmp/pi-b.json https://example.com",
          "/tmp/pi-b.json",
        ],
        ["wget -q -O /tmp/pi-c.json https://example.com", "/tmp/pi-c.json"],
        [
          "wget -q --output-document /tmp/pi-d.json https://example.com",
          "/tmp/pi-d.json",
        ],
        ["curl -sS https://example.com > /tmp/pi-e.html", "/tmp/pi-e.html"],
        ["curl -sS https://example.com >> /tmp/pi-f.log", "/tmp/pi-f.log"],
        [
          "curl -sS https://api.example.com/v1 -o '/tmp/pi quoted.json'",
          "/tmp/pi quoted.json",
        ],
        [`wget -q -O "pi output.html" https://example.com`, "pi output.html"],
      ]) {
        const outputSession = `output-${path}`;
        assert.equal(await call("bash", { command }, outputSession), undefined);
        const result = await call("read", { path }, outputSession);
        assert.equal(result?.block, true, `${command} must taint ${path}`);
      }
      await deny(
        "bash",
        { command: "curl -sS --output=/tmp/pi-g.json https://example.com" },
        "quill_fetch_and_index",
      );
      for (const command of [
        "curl -sS -o /dev/null https://example.com",
        "curl -I https://example.com",
      ]) {
        const cleanSession = `clean-${command}`;
        assert.equal(await call("bash", { command }, cleanSession), undefined);
        assert.equal(
          await call(
            "read",
            { path: "https://example.com/a.tgz" },
            cleanSession,
          ),
          undefined,
        );
      }
      for (const command of [
        "curl -sSO https://example.com/a.tgz",
        "curl -O https://example.com/a.tgz",
      ]) {
        await deny("bash", { command }, "quill_fetch_and_index");
      }

      const sid = "pi-taint-round-trip";
      assert.equal(
        await call(
          "bash",
          {
            command:
              "curl -sS -o /tmp/quill-pi-router.json https://api.example.com/foo",
          },
          sid,
        ),
        undefined,
      );
      const taintedBash = await call(
        "bash",
        { command: "jq . /tmp/quill-pi-router.json" },
        sid,
      );
      assert.equal(taintedBash?.block, true);
      assert.match(taintedBash.reason, /earlier curl\/wget/);
      const taintedRead = await call(
        "read",
        { path: "/tmp/quill-pi-router.json" },
        sid,
      );
      assert.equal(taintedRead?.block, true);
      assert.match(taintedRead.reason, /quill_search_context/);

      for (const command of [
        "rm -f /tmp/quill-pi-router.json",
        "bash /tmp/quill-pi-router.json",
      ]) {
        assert.equal(await call("bash", { command }, sid), undefined);
      }
      assert.equal(
        await call("read", { path: "/tmp/not-tainted.txt" }, sid),
        undefined,
      );

      const quotedRead = await call(
        "bash",
        { command: "cat '/tmp/quill-pi-router.json'" },
        sid,
      );
      assert.equal(quotedRead?.block, true);
      assert.equal(
        await call(
          "bash",
          { command: "cat /tmp/quill-pi-router.json.bak" },
          sid,
        ),
        undefined,
      );
      assert.equal(
        await call(
          "bash",
          { command: "echo 'next: cat /tmp/quill-pi-router.json'" },
          sid,
        ),
        undefined,
      );
      assert.equal(
        await call(
          "read",
          { path: "/tmp/quill-pi-router.json" },
          "pi-other-session",
        ),
        undefined,
      );

      const boundedSession = "pi-bounded-taint";
      for (let index = 0; index < 260; index += 1) {
        await call(
          "bash",
          {
            command: `curl -sS -o /tmp/pi-bounded-${index}.json https://example.com`,
          },
          boundedSession,
        );
      }
      const state = JSON.parse(
        readFileSync(
          join(
            process.env.HOME,
            ".config",
            "quill",
            "context",
            "markers",
            `pi-${boundedSession}`,
            "tainted.json",
          ),
          "utf8",
        ),
      );
      assert.equal(state.paths.length, 256);
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Ready URL rewrites]]
test("every URL block has a nonempty ready-to-paste Pi rewrite", async (t) => {
  const oldFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ body: { cancel: async () => {} } });
  t.after(() => {
    globalThis.fetch = oldFetch;
  });
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const route = routingHandler(pi);
      for (const [toolName, input, urls] of [
        [
          "web_fetch",
          { url: "https://example.com/docs" },
          ["https://example.com/docs"],
        ],
        [
          "fetch_content",
          { url: "https://example.com/article" },
          ["https://example.com/article"],
        ],
        [
          "fetch_content",
          { urls: ["https://example.com/one", "https://example.com/two"] },
          ["https://example.com/one", "https://example.com/two"],
        ],
        [
          "bash",
          { command: "curl https://api.example.com/v1/items.json" },
          ["https://api.example.com/v1/items.json"],
        ],
      ]) {
        const result = await route(
          { type: "tool_call", toolName, toolCallId: "call", input },
          context("pi-rewrite"),
        );
        assert.equal(result?.block, true);
        assert.ok(result.reason?.trim());
        for (const url of urls) {
          assert.ok(
            result.reason.includes(
              `quill_fetch_and_index(url=${JSON.stringify(url)})`,
            ),
            result.reason,
          );
        }
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Routing feature gate]]
test("context preservation off registers no router or routing telemetry", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const source = readFileSync(
        join(import.meta.dirname, "quill.ts"),
        "utf8",
      );
      const path = join(process.env.HOME, "context-off.mjs");
      writeFileSync(
        path,
        renderFeatures(source, {
          context_preservation: false,
          activity_tracking: false,
          context_telemetry: true,
        }),
      );
      const rendered = (await import(pathToFileURL(path).href)).default;
      process.env.QUILL_PI_REPORTER_PATH = path;
      const pi = fakePi();
      let requests = 0;
      const oldFetch = globalThis.fetch;
      globalThis.fetch = () => {
        requests += 1;
        return Promise.reject(new Error("unexpected request"));
      };
      try {
        rendered(pi.api);
        assert.equal(pi.tools.size, 0);
        assert.deepEqual([...pi.handlers.keys()], ["session_shutdown"]);
        assert.equal(requests, 0);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Routing telemetry]]
test("routing telemetry posts Pi routing events with zero token fields", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret", hostname: "pi-host" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const payloads = [];
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async (url, options) => {
        if (String(url).endsWith("/api/v1/context-savings/events")) {
          payloads.push(JSON.parse(options.body));
        }
        return { body: { cancel: async () => {} } };
      };
      try {
        const result = await routingHandler(pi)(
          {
            type: "tool_call",
            toolName: "bash",
            toolCallId: "call",
            input: { command: "curl https://example.com/docs" },
          },
          context("pi-routing-telemetry"),
        );
        assert.equal(result.block, true);
        await new Promise((resolve) => setImmediate(resolve));
        assert.equal(payloads.length, 1);
        const event = payloads[0].events[0];
        assert.equal(event.provider, "pi");
        assert.equal(event.category, "routing");
        assert.equal(event.eventType, "router.denial");
        assert.equal(event.sessionId, "pi-routing-telemetry");
        for (const field of [
          "tokensIndexedEst",
          "tokensReturnedEst",
          "tokensSavedEst",
          "tokensPreservedEst",
        ]) {
          assert.equal(event[field], 0, `${field} must be zero`);
        }
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Routing telemetry containment]]
test("routing telemetry failure never changes the deny decision", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      const oldFetch = globalThis.fetch;
      globalThis.fetch = () => {
        throw new Error("down");
      };
      try {
        const result = await routingHandler(pi)(
          {
            type: "tool_call",
            toolName: "bash",
            toolCallId: "call",
            input: { command: "curl https://example.com" },
          },
          context("pi-telemetry-down"),
        );
        assert.equal(result.block, true);
        assert.ok(result.reason);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

test("context telemetry off preserves routing without posting", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const source = readFileSync(
        join(import.meta.dirname, "quill.ts"),
        "utf8",
      );
      const path = join(process.env.HOME, "telemetry-off.mjs");
      writeFileSync(
        path,
        renderFeatures(source, {
          context_preservation: true,
          activity_tracking: false,
          context_telemetry: false,
        }),
      );
      const rendered = (await import(pathToFileURL(path).href)).default;
      process.env.QUILL_PI_REPORTER_PATH = path;
      const pi = fakePi();
      let requests = 0;
      const oldFetch = globalThis.fetch;
      globalThis.fetch = () => {
        requests += 1;
        return Promise.resolve({ body: { cancel: async () => {} } });
      };
      try {
        rendered(pi.api);
        const result = await routingHandler(pi)(
          {
            type: "tool_call",
            toolName: "bash",
            toolCallId: "call",
            input: { command: "curl https://example.com" },
          },
          context("pi-telemetry-off"),
        );
        assert.equal(result.block, true);
        assert.equal(requests, 0);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Sustained event load]]
test("sustains 1000 events per minute without turn delay or unbounded RSS", async () => {
  const minutes = Number(process.env.QUILL_PI_LOAD_MINUTES || "0.01");
  const total = Math.max(1, Math.round(minutes * 1000));
  const intervalMs = (minutes * 60_000) / total;
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    async () => {
      const pi = fakePi();
      quill(pi.api);
      let requests = 0;
      let maxHandlerMs = 0;
      const rssStart = process.memoryUsage().rss;
      let rssMax = rssStart;
      const oldFetch = globalThis.fetch;
      globalThis.fetch = async () => {
        requests += 1;
        return { ok: true, status: 202, body: { cancel: async () => {} } };
      };
      try {
        await new Promise((resolve) => {
          let sent = 0;
          const timer = setInterval(() => {
            const started = performance.now();
            pi.handlers.get("model_select")[0](
              {
                type: "model_select",
                model: { provider: "probe", id: `model-${sent % 2}` },
                source: "set",
              },
              context("load-session"),
            );
            maxHandlerMs = Math.max(maxHandlerMs, performance.now() - started);
            rssMax = Math.max(rssMax, process.memoryUsage().rss);
            sent += 1;
            if (sent === total) {
              clearInterval(timer);
              resolve();
            }
          }, intervalMs);
        });
        await flushRequests();
        assert.equal(requests, total);
        assert.ok(
          maxHandlerMs < 10,
          `max handler delay ${maxHandlerMs.toFixed(3)}ms`,
        );
        assert.ok(
          rssMax - rssStart < 64 * 1024 * 1024,
          `RSS grew ${rssMax - rssStart} bytes`,
        );
        console.log(
          `PI_LOAD_RESULT minutes=${minutes} events=${total} max_handler_ms=${maxHandlerMs.toFixed(3)} rss_delta=${rssMax - rssStart}`,
        );
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Real Pi session]]
test("Pi 0.84.2 loads tracking and calls a Quill tool in an isolated session", async () => {
  const piBin = process.env.QUILL_PI_BIN || "pi";
  const piVersion = execFileSync(piBin, ["--version"], {
    encoding: "utf8",
  }).trim();
  assert.equal(piVersion, "0.84.2");
  const root = mkdtempSync(join(tmpdir(), "quill-real-pi-"));
  const configDir = join(root, "pi-agent");
  const sessionDir = join(root, "sessions");
  const quillConfigDir = join(root, ".config", "quill");
  mkdirSync(configDir, { recursive: true });
  mkdirSync(sessionDir, { recursive: true });
  mkdirSync(quillConfigDir, { recursive: true });

  let modelCalls = 0;
  let contextCalls = 0;
  const trackBodies = [];
  const runtimeBodies = [];
  const server = createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    if (request.url === "/v1/chat/completions") {
      modelCalls += 1;
      response.writeHead(200, {
        "Content-Type": "text/event-stream",
        Connection: "keep-alive",
      });
      const choice =
        modelCalls === 1
          ? {
              index: 0,
              delta: {
                role: "assistant",
                tool_calls: [
                  {
                    index: 0,
                    id: "quill-probe-call",
                    type: "function",
                    function: { name: "quill_context_stats", arguments: "{}" },
                  },
                ],
              },
              finish_reason: "tool_calls",
            }
          : {
              index: 0,
              delta: { role: "assistant", content: "Quill probe complete." },
              finish_reason: "stop",
            };
      response.write(
        `data: ${JSON.stringify({ id: `probe-${modelCalls}`, choices: [choice] })}\n\n`,
      );
      response.write(
        `data: ${JSON.stringify({ choices: [], usage: { prompt_tokens: 10, completion_tokens: 2, total_tokens: 12 } })}\n\n`,
      );
      response.end("data: [DONE]\n\n");
      return;
    }
    if (request.url === "/api/v1/context/stats") {
      contextCalls += 1;
      assert.equal(request.headers.authorization, "Bearer real-pi-secret");
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end(JSON.stringify({ sources: 7, chunks: 11 }));
      return;
    }
    if (request.url === "/api/v1/pi/track") {
      trackBodies.push(JSON.parse(body));
      response.writeHead(202, { "Content-Type": "application/json" });
      response.end("{}");
      return;
    }
    if (request.url === "/api/v1/sessions/messages") {
      runtimeBodies.push(JSON.parse(body));
      response.writeHead(202, { "Content-Type": "application/json" });
      response.end("{}");
      return;
    }
    if (request.url === "/api/v1/sessions/notify") {
      response.writeHead(202, { "Content-Type": "application/json" });
      response.end("{}");
      return;
    }
    if (request.url === "/api/v1/hooks/observed") {
      response.writeHead(202);
      response.end("queued");
      return;
    }
    response.writeHead(404);
    response.end();
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  writeFileSync(
    join(configDir, "models.json"),
    JSON.stringify({
      providers: {
        "quill-probe": {
          baseUrl: `${baseUrl}/v1`,
          api: "openai-completions",
          apiKey: "probe",
          models: [{ id: "probe" }],
        },
      },
    }),
  );
  writeFileSync(
    join(quillConfigDir, "config.json"),
    JSON.stringify({
      url: baseUrl,
      context_url: baseUrl,
      secret: "real-pi-secret",
      hostname: "pi-test",
      pi_reporter: REPORTER_CONTRACT,
    }),
  );

  const started = performance.now();
  const child = spawn(
    piBin,
    [
      "--offline",
      "--provider",
      "quill-probe",
      "--model",
      "probe",
      "--api-key",
      "probe",
      "--mode",
      "json",
      "--print",
      "--session-dir",
      sessionDir,
      "--no-context-files",
      "--no-skills",
      "--no-extensions",
      "--extension",
      join(import.meta.dirname, "quill.ts"),
      "--tools",
      "quill_context_stats",
      "Call quill_context_stats once, then stop.",
    ],
    {
      env: {
        ...process.env,
        HOME: root,
        PI_CODING_AGENT_DIR: configDir,
        PI_CODING_AGENT_SESSION_DIR: sessionDir,
        PI_SUBAGENT_CHILD: "0",
        QUILL_PI_REPORTER_PATH: SOURCE_PATH,
      },
    },
  );
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => (stdout += chunk));
  child.stderr.on("data", (chunk) => (stderr += chunk));
  child.stdin.end();
  const timer = setTimeout(() => child.kill("SIGTERM"), 15000);
  const [status, signal] = await once(child, "exit");
  clearTimeout(timer);
  const sessionMs = Math.round((performance.now() - started) * 10) / 10;

  try {
    assert.equal(
      status,
      0,
      JSON.stringify({ signal, modelCalls, contextCalls, stdout, stderr }),
    );
    assert.equal(modelCalls, 2);
    assert.equal(contextCalls, 1);
    const trackEvents = trackBodies.flatMap((body) => body.events);
    assert.ok(
      trackEvents.some(
        (event) => (event.event || event.type) === "session_start",
      ),
    );
    assert.ok(trackEvents.some((event) => event.type === "usage"));
    assert.ok(
      trackEvents.some(
        (event) => (event.event || event.type) === "session_end",
      ),
    );
    assert.ok(
      runtimeBodies
        .flatMap((body) => body.messages)
        .some((message) => message.role === "assistant"),
    );
    assert.match(stdout, /Quill probe complete/);
    const toolResult = stdout
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line))
      .find(
        (record) =>
          record.type === "message_end" &&
          record.message?.role === "toolResult",
      );
    assert.equal(JSON.parse(toolResult.message.content[0].text).sources, 7);
    assert.deepEqual(toolResult.message.details, { ok: true });
    const sessionFile = readdirSync(sessionDir, { recursive: true }).find(
      (entry) => entry.endsWith(".jsonl"),
    );
    assert.ok(sessionFile);
    const persisted = readFileSync(join(sessionDir, sessionFile), "utf8")
      .trimEnd()
      .split("\n")
      .map((line) => JSON.parse(line));
    const trackingEntries = persisted.filter(
      (entry) =>
        entry.type === "custom" && entry.customType === "quill-tracking",
    );
    assert.ok(
      trackingEntries.some((entry) => entry.data.event === "session_start"),
    );
    assert.ok(
      trackingEntries.every((entry) =>
        trackEvents.some((event) => event.event_uuid === entry.data.event_uuid),
      ),
    );
    console.log(
      `REAL_PI_RESULT version=${piVersion} context_calls=1 session_ms=${sessionMs}`,
    );
  } finally {
    server.close();
    await once(server, "close");
    rmSync(root, { recursive: true, force: true });
  }
});
