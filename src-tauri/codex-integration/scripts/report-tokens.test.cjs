#!/usr/bin/env node
"use strict";

const assert = require("node:assert/strict");
const { mkdtempSync, rmSync, writeFileSync } = require("node:fs");
const http = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const test = require("node:test");

const { postJSON } = require("./lib.cjs");
const { buildPayload, findLastUsage, validatedUsage } = require("./report-tokens.cjs");

test("reverse scan recovers the newest valid token_count event", () => {
  const dir = mkdtempSync(join(tmpdir(), "quill-report-tokens-"));
  const transcript = join(dir, "rollout.jsonl");
  try {
    writeFileSync(transcript, [
      JSON.stringify({
        type: "event_msg",
        payload: { type: "token_count", info: { last_token_usage: { input_tokens: 1, output_tokens: 1 } } },
      }),
      JSON.stringify({
        type: "event_msg",
        payload: {
          type: "token_count",
          info: {
            last_token_usage: { input_tokens: true },
            total_token_usage: { input_tokens: 12, cached_input_tokens: 6, output_tokens: 34 },
          },
        },
      }),
      JSON.stringify({ type: "event_msg", payload: { type: "agent_message" } }),
      "{malformed",
      "",
    ].join("\n"));

    assert.deepEqual(findLastUsage(transcript), {
      input_tokens: 12,
      cached_input_tokens: 6,
      output_tokens: 34,
    });
  } finally {
    rmSync(dir, { force: true, recursive: true });
  }
});

test("usage validation rejects empty, boolean, and negative counts", () => {
  assert.equal(validatedUsage({}), null);
  assert.equal(validatedUsage({ input_tokens: true }), null);
  assert.equal(validatedUsage({ output_tokens: -1 }), null);
  assert.deepEqual(validatedUsage({ input_tokens: 5 }), {
    input_tokens: 5,
    cached_input_tokens: 0,
    output_tokens: 0,
  });
});

test("payload maps cached input tokens and keeps cwd optional", () => {
  const usage = { input_tokens: 12, cached_input_tokens: 6, output_tokens: 34 };
  assert.deepEqual(
    buildPayload({ session_id: "s", cwd: "/project" }, { hostname: "configured-host" }, usage),
    {
      provider: "codex",
      session_id: "s",
      hostname: "configured-host",
      input_tokens: 12,
      output_tokens: 34,
      cache_creation_input_tokens: 0,
      cache_read_input_tokens: 6,
      cwd: "/project",
    },
  );
  assert.ok(!("cwd" in buildPayload({ session_id: "s" }, {}, usage)));
});

test("postJSON posts an authorized JSON body and swallows failures", async () => {
  const requests = [];
  const server = http.createServer((request, response) => {
    let body = "";
    request.on("data", (chunk) => { body += chunk; });
    request.on("end", () => {
      requests.push({
        url: request.url,
        auth: request.headers.authorization,
        contentType: request.headers["content-type"],
        body: JSON.parse(body),
      });
      response.statusCode = 400;
      response.end();
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const config = { url: `http://127.0.0.1:${server.address().port}`, secret: "hook-secret" };
    await postJSON(config, "/api/v1/tokens", { provider: "codex" }, "test");
    assert.deepEqual(requests, [{
      url: "/api/v1/tokens",
      auth: "Bearer hook-secret",
      contentType: "application/json",
      body: { provider: "codex" },
    }]);
    // Unreachable target must resolve without throwing.
    await postJSON({ url: "http://127.0.0.1:1", secret: "x" }, "/nope", {}, "test");
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});
