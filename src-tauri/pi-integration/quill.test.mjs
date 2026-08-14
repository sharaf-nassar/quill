import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import quill from "./quill.ts";

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
    writeFileSync(path, typeof config === "string" ? config : JSON.stringify(config));
  }
  return root;
}

function fakePi({ registerError } = {}) {
  const tools = new Map();
  const handlers = new Map();
  let registrationAttempts = 0;
  return {
    api: {
      registerTool(tool) {
        registrationAttempts += 1;
        if (registerError) throw registerError;
        tools.set(tool.name, tool);
      },
      on(event, handler) {
        handlers.set(event, handler);
      },
    },
    handlers,
    tools,
    registrationAttempts: () => registrationAttempts,
  };
}

function context(sessionId = "pi-session") {
  return {
    cwd: "/tmp/project",
    sessionManager: { getSessionId: () => sessionId },
  };
}

async function withHome(config, run) {
  const root = configRoot(config);
  const oldHome = process.env.HOME;
  process.env.HOME = root;
  try {
    return await run();
  } finally {
    if (oldHome === undefined) delete process.env.HOME;
    else process.env.HOME = oldHome;
    rmSync(root, { recursive: true, force: true });
  }
}

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Self disabling load]]
test("missing, malformed, and remote config leave Pi unchanged", async () => {
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
      const source = readFileSync(join(import.meta.dirname, "quill.ts"), "utf8");
      for (const [flags, expectedTools, expectedHandlers] of [
        [{ context_preservation: false, activity_tracking: true }, 0, 8],
        [{ context_preservation: true, activity_tracking: false }, 8, 0],
      ]) {
        const path = join(
          process.env.HOME,
          `rendered-${flags.context_preservation}-${flags.activity_tracking}.mjs`,
        );
        writeFileSync(
          path,
          source.replace(
            "const FEATURES = { context_preservation: true, activity_tracking: true };",
            `const FEATURES = { context_preservation: ${flags.context_preservation}, activity_tracking: ${flags.activity_tracking} };`,
          ),
        );
        const rendered = (await import(pathToFileURL(path).href)).default;
        const pi = fakePi();
        rendered(pi.api);
        assert.equal(pi.tools.size, expectedTools);
        assert.equal(pi.handlers.size, expectedHandlers);
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

      const pi = fakePi();
      quill(pi.api);
      const oldFetch = globalThis.fetch;
      globalThis.fetch = () => {
        throw new Error("down");
      };
      try {
        for (const tool of pi.tools.values()) {
          const result = await tool.execute("call", {}, undefined, undefined, context());
          assert.equal(result.isError, true);
          assert.equal(result.details.ok, false);
          assert.equal(result.details.error.type, "quill_unavailable");
          assert.ok(result.content[0].text.length < 80);
        }
        for (const handler of pi.handlers.values()) {
          assert.doesNotThrow(() =>
            handler({}, { cwd: "/tmp", sessionManager: { getSessionId: () => { throw new Error("bad session"); } } }),
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
        const history = await pi.tools.get("quill_search_history").execute(
          "history",
          { query: "needle", project: "/tmp/project", limit: 3 },
          undefined,
          undefined,
          context(),
        );
        const indexed = await pi.tools.get("quill_index_context").execute(
          "index",
          { content: "large output" },
          undefined,
          undefined,
          context(),
        );
        assert.equal(history.isError, undefined);
        assert.equal(indexed.details.ok, true);
        assert.match(calls[0].url, /^http:\/\/\[::1\]:19876\/api\/v1\/sessions\/search\?/);
        assert.match(calls[0].url, /q=needle/);
        assert.match(calls[0].url, /page_size=3/);
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

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Bounded synchronous work]]
test("tool and telemetry handlers return from synchronous work in single-digit milliseconds", async () => {
  await withHome(
    { url: "http://127.0.0.1:19876", secret: "secret" },
    () => {
      const pi = fakePi();
      quill(pi.api);
      const oldFetch = globalThis.fetch;
      globalThis.fetch = () => new Promise(() => {});
      try {
        let started = performance.now();
        void pi.tools.get("quill_context_stats").execute("stats", {}, undefined, undefined, context());
        assert.ok(performance.now() - started < 10);
        started = performance.now();
        pi.handlers.get("turn_end")({}, context());
        assert.ok(performance.now() - started < 10);
      } finally {
        globalThis.fetch = oldFetch;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Telemetry mapping and timeout]]
test("telemetry maps Pi events and uses the shared local timeout", async () => {
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
      const timeouts = [];
      const oldFetch = globalThis.fetch;
      const oldTimeout = AbortSignal.timeout;
      globalThis.fetch = async (_url, options) => {
        payloads.push(JSON.parse(options.body));
        return { body: { cancel: async () => {} } };
      };
      AbortSignal.timeout = (milliseconds) => {
        timeouts.push(milliseconds);
        return oldTimeout(milliseconds);
      };
      try {
        const mapping = {
          session_start: "SessionStart",
          input: "UserPromptSubmit",
          tool_call: "PreToolUse",
          tool_result: "PostToolUse",
          turn_end: "Stop",
          session_shutdown: "SessionEnd",
          session_before_compact: "PreCompact",
          session_compact: "PostCompact",
        };
        for (const event of Object.keys(mapping)) {
          pi.handlers.get(event)({ toolName: "read" }, context());
        }
        await new Promise((resolve) => setImmediate(resolve));
        assert.deepEqual(payloads.map((payload) => payload.hook_event), Object.values(mapping));
        assert.ok(payloads.every((payload) => payload.provider === "pi"));
        assert.ok(payloads.every((payload) => payload.session_id === "pi-session"));
        assert.ok(payloads.every((payload) => payload.hostname === "pi-host"));

        const lib = readFileSync(
          join(import.meta.dirname, "..", "codex-integration", "scripts", "lib.cjs"),
          "utf8",
        );
        const sharedTimeout = Number(lib.match(/const LOCAL_TIMEOUT_MS = (\d+);/)[1]);
        assert.ok(timeouts.length >= Object.keys(mapping).length);
        assert.ok(timeouts.every((timeout) => timeout === sharedTimeout));
      } finally {
        globalThis.fetch = oldFetch;
        AbortSignal.timeout = oldTimeout;
      }
    },
  );
});

// @lat: [[pi-extension-tests#Pi Extension Test Specs#Real Pi session]]
test("Pi 0.84.1 loads and calls a Quill tool in an isolated session", async () => {
  const piBin = "/home/mamba/.nvm/versions/node/v25.8.2/bin/pi";
  const root = mkdtempSync(join(tmpdir(), "quill-real-pi-"));
  const configDir = join(root, "pi-agent");
  const sessionDir = join(root, "sessions");
  const quillConfigDir = join(root, ".config", "quill");
  mkdirSync(configDir, { recursive: true });
  mkdirSync(sessionDir, { recursive: true });
  mkdirSync(quillConfigDir, { recursive: true });

  let modelCalls = 0;
  let contextCalls = 0;
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
      response.write(`data: ${JSON.stringify({ id: `probe-${modelCalls}`, choices: [choice] })}\n\n`);
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
    assert.match(stdout, /Quill probe complete/);
    assert.match(stdout, /"sources":7/);
    assert.ok(readdirSync(sessionDir, { recursive: true }).some((entry) => entry.endsWith(".jsonl")));
    console.log(`REAL_PI_RESULT version=0.84.1 context_calls=1 session_ms=${sessionMs}`);
  } finally {
    server.close();
    await once(server, "close");
    rmSync(root, { recursive: true, force: true });
  }
});
