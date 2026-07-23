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
  const capture = require("./context-capture.cjs");
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

process.stdout.write(`\n${passed} passed, ${failed} failed\n`);
process.exit(failed === 0 ? 0 : 1);
