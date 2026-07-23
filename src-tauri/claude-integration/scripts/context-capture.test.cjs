#!/usr/bin/env node
"use strict";

// Standalone fixtures for context-capture.cjs. No test runner required:
//   node context-capture.test.cjs
// Exits 0 on success, 1 with diagnostics on failure.

const childProcess = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const scriptPath = path.join(__dirname, "context-capture.cjs");
const capture = require("./context-capture.cjs");
let passed = 0;
let failed = 0;

function it(name, fn) {
  try {
    fn();
    passed += 1;
    process.stdout.write(`  ok  ${name}\n`);
  } catch (err) {
    failed += 1;
    process.stdout.write(`  FAIL ${name}\n    ${err.message}\n`);
  }
}

function assert(condition, message) {
  if (!condition) throw new Error(message || "assertion failed");
}

function jsonLine(record) {
  return `${JSON.stringify(record)}\n`;
}

function continuityPath(home) {
  return path.join(home, ".config", "quill", "context", "continuity");
}

function seedRecords(home, records) {
  const dir = continuityPath(home);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "events.jsonl"), records.map(jsonLine).join(""), "utf8");
}

function seedSnapshots(home, records) {
  const dir = continuityPath(home);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "snapshots.jsonl"), records.map(jsonLine).join(""), "utf8");
}

function event(extra) {
  return {
    kind: "event",
    timestamp: "2026-07-23T12:00:00.000Z",
    provider: "claude",
    session_id: "prior-session",
    cwd: "/project",
    hook_event: "UserPromptSubmit",
    prompt_summary: null,
    hints: { decisions: [], tasks: [] },
    ...extra,
  };
}

function runCapture(home, input) {
  const result = childProcess.spawnSync(process.execPath, [scriptPath], {
    input: JSON.stringify(input),
    encoding: "utf8",
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      QUILL_DEBUG: "1",
    },
  });
  assert(result.status === 0, `capture exited ${result.status}: ${result.stderr}`);
  assert(result.stderr === "", `capture wrote debug output: ${result.stderr}`);
  return result.stdout.trim() ? JSON.parse(result.stdout) : null;
}

function withFixture(fn) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "quill-capture-test-"));
  try {
    fn(home);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
}

function directive(output) {
  return output?.hookSpecificOutput?.additionalContext || "";
}

it("spawns stdin JSON and emits a SessionStart continuity directive", () => withFixture((home) => {
  seedRecords(home, [event({
    cwd: "/project",
    prompt_summary: "Implement the fixture harness.",
    hints: { decisions: ["Prefer deterministic fixtures."], tasks: ["Build the harness."] },
  })]);

  const output = runCapture(home, {
    hook_event_name: "SessionStart",
    provider: "claude",
    session_id: "new-session",
    cwd: "/project",
  });
  const text = directive(output);
  assert(text.includes("last_prompt: Implement the fixture harness."), `missing prompt:\n${text}`);
  assert(text.includes("task_hints: Build the harness."), `missing task hints:\n${text}`);
  assert(text.includes("decision_hints: Prefer deterministic fixtures."), `missing decisions:\n${text}`);
}));

it("preserves the empty directive gate", () => withFixture((home) => {
  seedRecords(home, [event({ cwd: "/project" })]);
  const output = runCapture(home, {
    hook_event_name: "SessionStart",
    provider: "claude",
    session_id: "new-session",
    cwd: "/project",
  });
  assert(output === null, `expected no directive, got ${JSON.stringify(output)}`);
}));

it("scopes main checkout and worktree .git-file projects separately", () => withFixture((home) => {
  const root = path.join(home, "repo");
  const worktree = path.join(root, ".worktrees", "feature");
  fs.mkdirSync(path.join(root, ".git"), { recursive: true });
  fs.mkdirSync(worktree, { recursive: true });
  fs.writeFileSync(path.join(worktree, ".git"), "gitdir: /elsewhere/worktrees/feature\n", "utf8");

  seedRecords(home, [
    event({
      cwd: root,
      prompt_summary: "Main checkout context.",
      hints: { decisions: [], tasks: ["Keep main only."] },
    }),
    event({
      cwd: worktree,
      timestamp: "2026-07-23T12:01:00.000Z",
      prompt_summary: "Worktree context.",
      hints: { decisions: [], tasks: ["Keep worktree only."] },
    }),
  ]);

  const main = directive(runCapture(home, {
    hook_event_name: "SessionStart", provider: "claude", session_id: "main-new", cwd: root,
  }));
  assert(main.includes("Main checkout context."), `main record missing:\n${main}`);
  assert(!main.includes("Worktree context."), `worktree record leaked into main:\n${main}`);

  const child = directive(runCapture(home, {
    hook_event_name: "SessionStart", provider: "claude", session_id: "worktree-new", cwd: worktree,
  }));
  assert(child.includes("Worktree context."), `worktree record missing:\n${child}`);
  assert(!child.includes("Main checkout context."), `main record leaked into worktree:\n${child}`);
}));

it("exercises the exported handler for ignored hook events", () => {
  assert(typeof capture.handleInput === "function", "handleInput must remain exported for direct fixtures");
  assert(capture.handleInput({ hook_event_name: "PreToolUse" }) === undefined,
    "unhandled hook events must not produce side effects");
});

it("classifies prompts at the triviality boundary", () => {
  const { isTrivialPrompt } = require("./context-capture.cjs");
  assert(isTrivialPrompt("abcdefghijk"), "11 characters must be trivial");
  assert(!isTrivialPrompt("abcde fghijk"), "12 characters with whitespace must be non-trivial");
  assert(isTrivialPrompt("twelveletters"), "single tokens must be trivial regardless of length");
});

it("omits trivial prompts from SessionStart directives", () => withFixture((home) => {
  seedRecords(home, [
    event({ timestamp: "2026-07-23T12:01:00.000Z", prompt_summary: "ctc" }),
    event({ prompt_summary: "Implement the continuity capture update." }),
  ]);
  const text = directive(runCapture(home, {
    hook_event_name: "SessionStart", provider: "claude", session_id: "new-session", cwd: "/project",
  }));
  assert(text.includes("last_prompt: Implement the continuity capture update."), `missing fallback prompt:\n${text}`);
  assert(!text.includes("last_prompt: ctc"), `trivial prompt leaked:\n${text}`);
}));

it("exports selection helpers for snapshot-first continuity sourcing", () => {
  const { selectAnchor, sourceHints } = require("./context-capture.cjs");
  const records = [
    event({
      kind: "snapshot",
      session_id: "anchor",
      prompt_summaries: ["Implement the context capture fixture."],
      tasks: ["Use snapshot task."],
      decisions: ["Prefer snapshot values."],
    }),
    event({
      session_id: "anchor",
      prompt_summary: "Implement the context capture fixture.",
      hints: { tasks: ["Use event fallback."], decisions: [] },
    }),
  ];
  assert(selectAnchor(records)?.session_id === "anchor", "must find coherent anchor");
  const sourced = sourceHints(records, "anchor");
  assert(sourced.source === "snapshot", "snapshot records must be preferred");
  assert(sourced.tasks[0] === "Use snapshot task.", "snapshot task must lead sourced hints");
});

it("atomically prunes while a concurrent aggregate append waits on the lock", () => withFixture((home) => {
  const filePath = path.join(continuityPath(home), "events.jsonl");
  const recent = event({ timestamp: new Date().toISOString(), prompt_summary: "Keep this record." });
  const appended = event({ timestamp: new Date().toISOString(), prompt_summary: "Concurrent append survives." });
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${jsonLine(event({ timestamp: "2020-01-01T00:00:00.000Z" }))}${jsonLine(recent)}`, "utf8");

  const signalPath = path.join(home, "append-started");
  const completePath = path.join(home, "append-complete");
  capture.pruneJsonlFile(filePath, Date.now() - 60_000, {
    onLocked: () => {
      const duringRename = fs.readFileSync(filePath, "utf8");
      assert(duringRename.includes(recent.prompt_summary), "reader saw a torn or empty file during prune");
      childProcess.spawn(process.execPath, ["-e", [
        "const fs = require('fs');",
        `fs.writeFileSync(${JSON.stringify(signalPath)}, 'started');`,
        `require(${JSON.stringify(scriptPath)}).appendJsonLine(${JSON.stringify(filePath)}, ${JSON.stringify(appended)});`,
        `fs.writeFileSync(${JSON.stringify(completePath)}, 'complete');`,
      ].join("")], { stdio: "ignore" });
      const deadline = Date.now() + 500;
      while (!fs.existsSync(signalPath) && Date.now() < deadline) {
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 5);
      }
      assert(fs.existsSync(signalPath), "concurrent append process did not start");
    },
  });
  const deadline = Date.now() + 1_000;
  while (!fs.existsSync(completePath) && Date.now() < deadline) {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 5);
  }
  assert(fs.existsSync(completePath), "concurrent append did not complete");
  const content = fs.readFileSync(filePath, "utf8");
  assert(content.includes(recent.prompt_summary), "prune lost retained record");
  assert(content.includes(appended.prompt_summary), "prune lost concurrent append");
  assert(content.endsWith("\n"), "reader observed a torn JSONL write");
}));

it("steals stale aggregate locks and fails open when a live lock cannot be acquired", () => withFixture((home) => {
  const filePath = path.join(continuityPath(home), "events.jsonl");
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(`${filePath}.lock`, JSON.stringify({ pid: 1, timestamp: "2020-01-01T00:00:00.000Z" }), "utf8");
  capture.appendJsonLine(filePath, event({ prompt_summary: "Stale lock is replaced." }));
  assert(!fs.existsSync(`${filePath}.lock`), "stale lock should be released after append");

  fs.writeFileSync(`${filePath}.lock`, JSON.stringify({ pid: 1, timestamp: new Date().toISOString() }), "utf8");
  capture.appendJsonLine(filePath, event({ prompt_summary: "Lock contention fails open." }));
  assert(fs.readFileSync(filePath, "utf8").includes("Lock contention fails open."), "capture must continue on lock contention");
  assert(fs.existsSync(`${filePath}.lock`), "live lock must remain held when capture fails open");
}));

it("anchors the 2026-05-18 mixed-hints shape to one coherent session", () => withFixture((home) => {
  seedRecords(home, [
    event({
      session_id: "thread-a",
      timestamp: "2026-07-23T12:02:00.000Z",
      prompt_summary: "Implement the newest unrelated task.",
    }),
    event({
      session_id: "thread-b",
      timestamp: "2026-07-23T12:01:00.000Z",
      prompt_summary: "Finish the coherent continuity migration.",
      hints: { tasks: ["Thread B task."], decisions: ["Thread B decision."] },
    }),
  ]);
  const text = directive(runCapture(home, {
    hook_event_name: "SessionStart", provider: "claude", session_id: "new-session", cwd: "/project",
  }));
  assert(text.includes("last_prompt: Finish the coherent continuity migration."), `wrong anchor:\n${text}`);
  assert(text.includes("Thread B task.") && text.includes("Thread B decision."), `missing coherent hints:\n${text}`);
  assert(!text.includes("Implement the newest unrelated task."), `mixed thread prompt leaked:\n${text}`);
}));

it("uses snapshot fields first and fills empty snapshot hints from events", () => withFixture((home) => {
  seedRecords(home, [event({
    session_id: "anchor",
    prompt_summary: "Implement the event fallback for missing hints.",
    hints: { tasks: ["Event task fallback."], decisions: ["Event decision fallback."] },
  })]);
  seedSnapshots(home, [{
    kind: "snapshot",
    timestamp: "2026-07-23T12:01:00.000Z",
    provider: "claude",
    session_id: "anchor",
    cwd: "/project",
    prompt_summaries: ["Implement the snapshot-first directive."],
    tasks: [],
    decisions: [],
  }]);
  const text = directive(runCapture(home, {
    hook_event_name: "SessionStart", provider: "claude", session_id: "new-session", cwd: "/project",
  }));
  assert(text.includes("last_prompt: Implement the snapshot-first directive."), `snapshot prompt missing:\n${text}`);
  assert(text.includes("Event task fallback.") && text.includes("Event decision fallback."),
    `empty snapshot did not degrade to event hints:\n${text}`);
  assert(!text.includes("Implement the event fallback for missing hints."), `event prompt beat snapshot:\n${text}`);
}));

it("omits last_prompt but retains recent hints when every prompt is trivial", () => withFixture((home) => {
  seedRecords(home, [event({
    prompt_summary: "continue",
    hints: { tasks: ["Resume captured work."], decisions: [] },
  })]);
  const text = directive(runCapture(home, {
    hook_event_name: "SessionStart", provider: "claude", session_id: "new-session", cwd: "/project",
  }));
  assert(!text.includes("last_prompt:"), `trivial prompt was selected:\n${text}`);
  assert(text.includes("task_hints: Resume captured work."), `recent hints missing:\n${text}`);
}));

process.stdout.write(`\n${passed} passed, ${failed} failed\n`);
process.exit(failed === 0 ? 0 : 1);
