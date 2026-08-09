#!/usr/bin/env node
"use strict";

const childProcess = require("child_process");
const crypto = require("crypto");
const fs = require("fs");
const http = require("http");
const os = require("os");
const path = require("path");

const observe = require("./observe.cjs");
const qbuild = require("./qbuild-guard.cjs");
const tokens = require("./report-tokens.cjs");
const sync = require("./session-sync.cjs");

let passed = 0;
let failed = 0;
const tests = [];

function assert(condition, message) {
  if (!condition) throw new Error(message || "assertion failed");
}

function it(name, fn) {
  tests.push({ name, fn });
}

async function withFixture(fn) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "quill-hook-test-"));
  try {
    await fn(root);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

function trackingPath(sessionId, transcriptPath) {
  const key = crypto
    .createHash("sha256")
    .update(`${sessionId}\0${path.resolve(transcriptPath)}`)
    .digest("hex")
    .slice(0, 24);
  return path.join(os.tmpdir(), `.quill-sync-${sessionId}-${key}`);
}

function runSyncHook(home, input) {
  return new Promise((resolve, reject) => {
    const child = childProcess.spawn(process.execPath, [path.join(__dirname, "session-sync.cjs")], {
      env: { ...process.env, HOME: home, USERPROFILE: home },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stderr = "";
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolve();
      else reject(new Error(`session-sync exited ${code}: ${stderr}`));
    });
    child.stdin.end(JSON.stringify(input));
  });
}

it("extracts the newest valid token usage across malformed trailing records", () => withFixture((root) => {
  const transcript = path.join(root, "session.jsonl");
  const expected = {
    input_tokens: 12,
    output_tokens: 34,
    cache_creation_input_tokens: 5,
    cache_read_input_tokens: 6,
  };
  fs.writeFileSync(transcript, [
    JSON.stringify({ type: "assistant", message: { usage: expected } }),
    JSON.stringify({ type: "user", message: { content: "x".repeat(70_000) } }),
    JSON.stringify({ type: "assistant", message: { usage: { input_tokens: true } } }),
    "{malformed",
    "",
  ].join("\n"));
  assert(JSON.stringify(tokens.findLastUsage(transcript)) === JSON.stringify(expected),
    "reverse scan did not recover the newest valid usage");
  assert(tokens.validatedUsage({}) === null, "empty usage must not become zero tokens");
  assert(tokens.validatedUsage({ output_tokens: -1 }) === null, "negative usage must be rejected");
}));

it("maps PostToolUseFailure details into a post observation", () => {
  const payload = observe.buildPayload({
    hook_event_name: "PostToolUseFailure",
    session_id: "session",
    tool_name: "Bash",
    tool_input: { command: "false" },
    error: "exit status 1",
    is_interrupt: true,
    duration_ms: 42,
    cwd: "/project",
  });
  const output = JSON.parse(payload.tool_output);
  assert(payload.hook_phase === "post", "failure event must use post phase");
  assert(output.error === "exit status 1", "failure error was lost");
  assert(output.is_interrupt === true && output.duration_ms === 42,
    "failure metadata was lost");
});

it("blocks qbuild edits by lexical and canonical containment", () => withFixture((root) => {
  const repository = path.join(root, "repository");
  const outside = path.join(root, "outside");
  fs.mkdirSync(repository);
  fs.mkdirSync(outside);
  const initialized = childProcess.spawnSync("git", ["init", repository], {
    encoding: "utf8",
    windowsHide: true,
  });
  assert(initialized.status === 0, `git init failed: ${initialized.stderr}`);
  fs.writeFileSync(path.join(repository, ".qbuild-lock.test"), "");

  const notebookInput = {
    cwd: repository,
    tool_input: { notebook_path: "analysis.ipynb" },
  };
  assert(qbuild.evaluate(notebookInput, "git") !== null,
    "NotebookEdit path inside main checkout must be denied");
  assert(qbuild.evaluate({
    cwd: repository,
    tool_input: { file_path: path.join(outside, "allowed.txt") },
  }, "git") === null, "ordinary outside path must remain allowed");

  const outsideLink = path.join(outside, "repository-link");
  const insideLink = path.join(repository, "outside-link");
  fs.symlinkSync(repository, outsideLink, process.platform === "win32" ? "junction" : "dir");
  fs.symlinkSync(outside, insideLink, process.platform === "win32" ? "junction" : "dir");
  assert(qbuild.evaluate({
    cwd: repository,
    tool_input: { file_path: path.join(outsideLink, "new.txt") },
  }, "git") !== null, "outside symlink into main checkout must be denied");
  assert(qbuild.evaluate({
    cwd: repository,
    tool_input: { file_path: path.join(insideLink, "new.txt") },
  }, "git") !== null, "inside symlink out of main checkout must be denied lexically");

  const result = childProcess.spawnSync(
    process.execPath,
    [path.join(__dirname, "qbuild-guard.cjs"), "git"],
    { input: JSON.stringify(notebookInput), encoding: "utf8" },
  );
  const denial = JSON.parse(result.stdout);
  assert(denial.hookSpecificOutput.permissionDecision === "deny",
    "qbuild guard must emit valid deny JSON");
}));

it("caps session sync by monotonic time and request count", () => {
  let now = 100;
  const budget = new sync.SyncBudget(() => now);
  for (let attempt = 0; attempt < 18; attempt += 1) {
    assert(budget.claimRequest() > 0, `request ${attempt + 1} was rejected early`);
  }
  assert(budget.claimRequest() === 0, "nineteenth request must be rejected");

  const expired = new sync.SyncBudget(() => now);
  now += 8001;
  assert(expired.claimRequest() === 0, "request after 8 seconds must be rejected");

  const messages = Array.from({ length: 501 }, (_, index) => ({
    message: { role: index === 499 ? "user" : "assistant" },
  }));
  assert(sync.firstMessageChunk(messages).length === 499,
    "500-message boundary must keep a user/assistant pair together");
});

it("syncs past a poisoned first row, retryable failure, and final multi-chunk tail", () => (
  withFixture(async (root) => {
    const configDir = path.join(root, ".config", "quill");
    fs.mkdirSync(configDir, { recursive: true });
    let trackingFile;
    let failNext = false;
    let requestCount = 0;
    let accepted = new Set();
    let cursorSamples = [];
    const server = http.createServer((request, response) => {
      let body = "";
      request.on("data", (chunk) => { body += chunk; });
      request.on("end", () => {
        requestCount += 1;
        cursorSamples.push(fs.existsSync(trackingFile)
          ? Number(fs.readFileSync(trackingFile, "utf8"))
          : 0);
        const payload = JSON.parse(body);
        if (failNext) {
          failNext = false;
          response.statusCode = 503;
          response.end("retry later");
          return;
        }
        if (payload.messages.some((message) => message.content === "poison")) {
          response.statusCode = 400;
          response.end("Invalid message content");
          return;
        }
        for (const message of payload.messages) accepted.add(message.uuid);
        response.statusCode = 200;
        response.end();
      });
    });
    await new Promise((resolve) => server.listen(0, "0.0.0.0", resolve));
    try {
      const port = server.address().port;
      fs.writeFileSync(path.join(configDir, "config.json"), JSON.stringify({
        url: `http://0.0.0.0:${port}`,
        secret: "test",
      }));
      const writeTranscript = (name, firstContent) => {
        const transcript = path.join(root, `${name}.jsonl`);
        const timestamp = new Date().toISOString();
        fs.writeFileSync(transcript, `${Array.from({ length: 501 }, (_, index) => JSON.stringify({
          type: "assistant",
          timestamp,
          message: { content: index === 0 ? firstContent : `message-${index}` },
        })).join("\n")}\n`);
        return transcript;
      };
      const invoke = async (sessionId, transcript) => {
        trackingFile = trackingPath(sessionId, transcript);
        await runSyncHook(root, {
          hook_event_name: "SessionEnd",
          session_id: sessionId,
          transcript_path: transcript,
          cwd: root,
        });
      };

      const poisonSession = `hook-test-${process.pid}-poison`;
      const poisonTranscript = writeTranscript("poison", "poison");
      await invoke(poisonSession, poisonTranscript);
      assert(requestCount === 18, `first-row isolation used ${requestCount} requests`);
      assert(accepted.size === 500, `expected 500 valid poison-run messages, got ${accepted.size}`);
      assert(Number(fs.readFileSync(trackingFile, "utf8")) === 501,
        "poison run did not advance through the second top-level chunk");
      assert(cursorSamples.includes(1), "poison row was not durably dropped before sibling sends");
      assert(cursorSamples.every((cursor, index) => index === 0 || cursor >= cursorSamples[index - 1]),
        `cursor regressed across poison bisection: ${cursorSamples.join(",")}`);

      requestCount = 0;
      accepted = new Set();
      cursorSamples = [];
      failNext = true;
      const retrySession = `hook-test-${process.pid}-retry`;
      const retryTranscript = writeTranscript("retry", "message-0");
      await invoke(retrySession, retryTranscript);
      assert(!fs.existsSync(trackingFile), "retryable failure must not advance cursor");
      await invoke(retrySession, retryTranscript);
      assert(requestCount === 3, `retry and two clean chunks used ${requestCount} requests`);
      assert(accepted.size === 501, `expected 501 retry-run messages, got ${accepted.size}`);
      assert(Number(fs.readFileSync(trackingFile, "utf8")) === 501,
        "retry run did not drain final multi-chunk tail");

      for (const [sessionId, transcript] of [
        [poisonSession, poisonTranscript],
        [retrySession, retryTranscript],
      ]) {
        const cursor = trackingPath(sessionId, transcript);
        fs.rmSync(cursor, { force: true });
        fs.rmSync(`${cursor}.leases`, { recursive: true, force: true });
      }
    } finally {
      await new Promise((resolve) => server.close(resolve));
    }
  })
));

async function run() {
  for (const { name, fn } of tests) {
    try {
      await fn();
      passed += 1;
      process.stdout.write(`  ok  ${name}\n`);
    } catch (error) {
      failed += 1;
      process.stdout.write(`  FAIL ${name}\n    ${error.message}\n`);
    }
  }
  process.stdout.write(`\n${passed} passed, ${failed} failed\n`);
  process.exit(failed === 0 ? 0 : 1);
}

void run();
