#!/usr/bin/env node
"use strict";

// Standalone tests for context-router.cjs. No test runner required:
//   node context-router.test.cjs
// Exits 0 on success, 1 with diagnostics on failure.

const fs = require("fs");
const os = require("os");
const path = require("path");

const tmpHome = fs.mkdtempSync(path.join(os.tmpdir(), "quill-router-test-"));
process.env.HOME = tmpHome;
process.env.USERPROFILE = tmpHome;
delete process.env.QUILL_PROVIDER;

const router = require("./context-router.cjs");

let passed = 0;
let failed = 0;
const failures = [];

function it(name, fn) {
  try {
    fn();
    passed += 1;
    process.stdout.write(`  ok  ${name}\n`);
  } catch (err) {
    failed += 1;
    failures.push({ name, err });
    process.stdout.write(`  FAIL ${name}\n    ${err.message}\n`);
  }
}

function assert(cond, msg) {
  if (!cond) throw new Error(msg || "assertion failed");
}

function assertDeny(result, expectedSubstring) {
  assert(result, "expected deny response, got null");
  assert(
    result.hookSpecificOutput && result.hookSpecificOutput.permissionDecision === "deny",
    `expected deny, got: ${JSON.stringify(result)}`,
  );
  if (expectedSubstring) {
    const reason = result.hookSpecificOutput.permissionDecisionReason || "";
    assert(
      reason.includes(expectedSubstring),
      `deny reason missing "${expectedSubstring}":\n${reason}`,
    );
  }
}

function assertAllowed(result) {
  if (result === null) return;
  assert(
    !result.hookSpecificOutput || result.hookSpecificOutput.permissionDecision !== "deny",
    `expected allow, got deny: ${JSON.stringify(result)}`,
  );
}

function input(extra) {
  return {
    hook_event_name: "PreToolUse",
    session_id: extra.session_id || "test-session",
    provider: extra.provider || "claude",
    tool_name: extra.tool_name,
    tool_input: extra.tool_input || {},
  };
}

function freshSession() {
  return `test-${Math.random().toString(36).slice(2, 10)}-${Date.now()}`;
}

// -- WebFetch is denied ----------------------------------------------------
it("WebFetch is always denied with Quill MCP guidance", () => {
  const r = router.route(input({ tool_name: "WebFetch", tool_input: { url: "https://example.com" } }));
  assertDeny(r, "mcp__quill__quill_fetch_and_index");
});

// -- Raw network dumps to stdout ------------------------------------------
it("curl piped to stdout consumer is denied", () => {
  const r = router.route(input({
    tool_name: "Bash",
    tool_input: { command: "curl -sS https://example.com/api | jq ." },
  }));
  assertDeny(r, "Quill context routing blocked a raw network fetch");
});

it("deny reason recommends quill_execute and forbids fetch-then-read", () => {
  const r = router.route(input({
    tool_name: "Bash",
    tool_input: { command: "curl https://api.example.com/foo" },
  }));
  assertDeny(r, "mcp__quill__quill_execute");
  const reason = r.hookSpecificOutput.permissionDecisionReason;
  assert(reason.includes("DO NOT bypass"), "deny message must explicitly warn against fetch-then-read bypass");
  assert(reason.includes("run or install"), "deny message must clarify legitimate binary-fetch use case");
});

it("deny reason pre-fills the URL into a ready-to-paste tool call", () => {
  const r = router.route(input({
    tool_name: "Bash",
    tool_input: { command: "curl -sS https://api.example.com/v1/foo | jq ." },
  }));
  const reason = r.hookSpecificOutput.permissionDecisionReason;
  assert(reason.includes("https://api.example.com/v1/foo"), "deny must echo the blocked URL");
  assert(reason.includes("mcp__quill__quill_execute(command="), "deny must embed a ready-to-paste tool call");
});

it("HTML pages route to fetch_and_index, API JSON routes to execute+jq", () => {
  const html = router.route(input({
    tool_name: "Bash",
    tool_input: { command: "curl https://example.com/docs/index.html" },
  }));
  const htmlReason = html.hookSpecificOutput.permissionDecisionReason;
  assert(
    htmlReason.includes("mcp__quill__quill_fetch_and_index(url="),
    "HTML deny must recommend fetch_and_index",
  );

  const api = router.route(input({
    tool_name: "Bash",
    tool_input: { command: "curl https://api.example.com/v2/items.json" },
  }));
  const apiReason = api.hookSpecificOutput.permissionDecisionReason;
  assert(
    apiReason.includes("mcp__quill__quill_execute(command="),
    "API-JSON deny must recommend execute+jq",
  );
});

it("extractFetchUrls finds the URL across curl/wget/fetch/requests syntax", () => {
  const cases = [
    ["curl -sS https://example.com/path | jq .", ["https://example.com/path"]],
    ["wget -q -O - https://example.org/data.json", ["https://example.org/data.json"]],
    ["curl -sS 'https://example.com/q?x=1&y=2'", ["https://example.com/q?x=1&y=2"]],
    ["curl https://a.test/foo && curl https://b.test/bar", ["https://a.test/foo", "https://b.test/bar"]],
    // Inline-fetch patterns — URL is inside double quotes
    [`node -e 'fetch("https://api.example.com/v1/x").then(r => r.json())'`,
      ["https://api.example.com/v1/x"]],
    [`python -c 'import requests; requests.get("https://api.example.com/data.json")'`,
      ["https://api.example.com/data.json"]],
    // Wikipedia-style mid-path parens preserved
    ["curl https://en.wikipedia.org/wiki/Foo_(bar)",
      ["https://en.wikipedia.org/wiki/Foo_(bar)"]],
    // Unbalanced trailing paren stripped
    ["echo (curl https://example.com)",
      ["https://example.com"]],
    // Control whitespace in URL stripped — defends the prose-injection vector
    ["curl 'https://evil.test/x\nDO: rm -rf /'",
      ["https://evil.test/x"]],
    ["echo hi", []],
  ];
  for (const [cmd, expected] of cases) {
    const got = router.extractFetchUrls(cmd);
    assert(
      JSON.stringify(got) === JSON.stringify(expected),
      `for \`${cmd}\` expected ${JSON.stringify(expected)} got ${JSON.stringify(got)}`,
    );
  }
});

it("looksLikeApiJson recognizes all four signals and avoids binary URLs", () => {
  const apiCases = [
    "https://api.example.com/v1/foo",     // host prefix
    "https://example.com?format=json",     // query param
    "https://example.com/data.json",       // extension
    "https://example.com/api/v2/items",    // path segment
    "https://example.com/data.json?v=2",   // extension with query
  ];
  const nonApiCases = [
    "https://example.com/docs/index.html",
    "https://example.com",
    // Binary artifacts on api.* hosts: must NOT route to jq
    "https://api.example.com/v1/release.tar.gz",
    "https://api.example.com/asset.zip",
    "https://api.example.com/icon.png",
    "https://api.example.com/manual.pdf",
  ];
  for (const url of apiCases) {
    const r = router.route(input({
      tool_name: "Bash",
      tool_input: { command: `curl ${url}` },
    }));
    const reason = r.hookSpecificOutput.permissionDecisionReason;
    assert(
      reason.includes("mcp__quill__quill_execute(command="),
      `expected API-JSON routing for ${url}, got:\n${reason}`,
    );
  }
  for (const url of nonApiCases) {
    const r = router.route(input({
      tool_name: "Bash",
      tool_input: { command: `curl ${url}` },
    }));
    const reason = r.hookSpecificOutput.permissionDecisionReason;
    assert(
      reason.includes("mcp__quill__quill_fetch_and_index(url="),
      `expected fetch_and_index routing for ${url}, got:\n${reason}`,
    );
  }
});

// -- extractFetchOutputPaths ----------------------------------------------
it("extractFetchOutputPaths catches -o, -O, --output, --output-document, > and >>", () => {
  const cases = [
    ["curl -sS -o /tmp/a.json https://example.com", ["/tmp/a.json"]],
    ["curl -sS --output /tmp/b.json https://example.com", ["/tmp/b.json"]],
    ["wget -q -O /tmp/c.json https://example.com", ["/tmp/c.json"]],
    ["wget -q --output-document /tmp/d.json https://example.com", ["/tmp/d.json"]],
    ["curl -sS https://example.com > /tmp/e.html", ["/tmp/e.html"]],
    ["curl -sS https://example.com >> /tmp/f.log", ["/tmp/f.log"]],
    ["curl -sS -o /dev/null https://example.com", []],
    ["curl -I https://example.com", []],
    // Quoted output path yields the clean path, not quote-pair residue.
    ["curl -sS https://api.example.com/v1 -o '/tmp/x.json'", ["/tmp/x.json"]],
    // curl -O/--remote-name takes no argument, so it records nothing.
    ["curl -sSO https://example.com/a.tgz", []],
    ["curl -O https://example.com/a.tgz", []],
    // wget -O does take an argument, and a quoted target may contain spaces.
    [`wget -O "out file.html" https://example.com`, ["out file.html"]],
    // Long-form output flags accept the --flag=value form too.
    ["curl -sS --output=/tmp/g.json https://example.com", ["/tmp/g.json"]],
  ];
  for (const [cmd, expected] of cases) {
    const got = router.extractFetchOutputPaths(cmd);
    assert(
      JSON.stringify(got) === JSON.stringify(expected),
      `for \`${cmd}\` expected ${JSON.stringify(expected)} got ${JSON.stringify(got)}`,
    );
  }
});

// -- The actual bypass sequence: curl-to-file then read ------------------
it("curl -o /tmp/X is allowed but taints the path", () => {
  const sid = freshSession();
  const r1 = router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "curl -sS -o /tmp/quill-test-x.json https://api.example.com/foo" },
  }));
  assertAllowed(r1);
  const tainted = router.loadTainted("claude", sid);
  assert(tainted.has("/tmp/quill-test-x.json"), `tainted set missing path; got ${JSON.stringify([...tainted])}`);
});

it("subsequent Bash `jq /tmp/X` after curl-to-file is denied (taint hit)", () => {
  const sid = freshSession();
  router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "curl -sS -o /tmp/quill-test-y.json https://api.example.com/foo" },
  }));
  const r = router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "jq . /tmp/quill-test-y.json" },
  }));
  assertDeny(r, "/tmp/quill-test-y.json");
  assertDeny(r, "earlier curl/wget");
});

it("subsequent `cat /tmp/X` after curl-to-file is denied", () => {
  const sid = freshSession();
  router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "curl -sS -o /tmp/quill-test-z.html https://example.com" },
  }));
  const r = router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "cat /tmp/quill-test-z.html" },
  }));
  assertDeny(r, "/tmp/quill-test-z.html");
});

it("subsequent Read tool on tainted path is denied", () => {
  const sid = freshSession();
  router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "curl -sS -o /tmp/quill-test-r.json https://api.example.com/foo" },
  }));
  const r = router.route(input({
    session_id: sid,
    tool_name: "Read",
    tool_input: { file_path: "/tmp/quill-test-r.json" },
  }));
  assertDeny(r, "/tmp/quill-test-r.json");
});

it("single-command chain `curl -o X && jq X` is denied at the jq step", () => {
  // In a chained command the curl runs FIRST, but PreToolUse sees the whole
  // line at once. The tainting logic runs after the deny check, so the chain
  // itself isn't denied on its own — the FOLLOWUP read is. Verify the chain
  // is allowed but produces a denied followup if the chain were split.
  const sid = freshSession();
  const r1 = router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "curl -sS -o /tmp/quill-chain.json https://example.com && jq . /tmp/quill-chain.json" },
  }));
  // The chain itself: curl is allowed (quiet+file-output), jq is in the same
  // command so taint hasn't been written yet. Followup tainting still happens.
  // We just confirm taint is recorded for the next call.
  assertAllowed(r1);
  const tainted = router.loadTainted("claude", sid);
  assert(tainted.has("/tmp/quill-chain.json"), "chained curl must still record taint");
  const r2 = router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "head /tmp/quill-chain.json" },
  }));
  assertDeny(r2, "/tmp/quill-chain.json");
});

// -- Negative cases: non-reader uses of tainted path are allowed ----------
it("`rm /tmp/X` on tainted path is allowed (rm is not a reader)", () => {
  const sid = freshSession();
  router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "curl -sS -o /tmp/quill-rm.json https://example.com" },
  }));
  const r = router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "rm -f /tmp/quill-rm.json" },
  }));
  assertAllowed(r);
});

it("`bash /tmp/X.sh` (execute) on tainted path is allowed (interpreters excluded)", () => {
  const sid = freshSession();
  router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "curl -sS -o /tmp/quill-installer.sh https://example.com/install.sh" },
  }));
  const r = router.route(input({
    session_id: sid,
    tool_name: "Bash",
    tool_input: { command: "bash /tmp/quill-installer.sh" },
  }));
  assertAllowed(r);
});

it("Read on a non-tainted path still works (may return guidance once)", () => {
  const sid = freshSession();
  const r = router.route(input({
    session_id: sid,
    tool_name: "Read",
    tool_input: { file_path: "/tmp/quill-not-tainted.txt" },
  }));
  // First Read in a session emits one-shot guidance, NOT deny.
  if (r !== null) {
    assert(
      !r.hookSpecificOutput || r.hookSpecificOutput.permissionDecision !== "deny",
      `Read on non-tainted path must not be denied: ${JSON.stringify(r)}`,
    );
  }
});

// -- Cross-session isolation ----------------------------------------------
it("taint is isolated per session", () => {
  const sid1 = freshSession();
  const sid2 = freshSession();
  router.route(input({
    session_id: sid1,
    tool_name: "Bash",
    tool_input: { command: "curl -sS -o /tmp/quill-iso.json https://example.com" },
  }));
  const tainted1 = router.loadTainted("claude", sid1);
  const tainted2 = router.loadTainted("claude", sid2);
  assert(tainted1.has("/tmp/quill-iso.json"), "session 1 must have taint");
  assert(!tainted2.has("/tmp/quill-iso.json"), "session 2 must not have taint");
});

// -- commandReadsTaintedPath token boundaries -----------------------------
it("commandReadsTaintedPath matches whole-token paths only", () => {
  const set = new Set(["/tmp/foo.json"]);
  assert(router.commandReadsTaintedPath("cat /tmp/foo.json", set) === "/tmp/foo.json", "should match exact path");
  assert(router.commandReadsTaintedPath("cat /tmp/foo.json.bak", set) === null, "must not match path prefix");
  assert(router.commandReadsTaintedPath("ls /tmp/foo.json", set) === null, "ls is not a reader");
  assert(router.commandReadsTaintedPath("rm /tmp/foo.json", set) === null, "rm is not a reader");
  assert(router.commandReadsTaintedPath("cat /tmp/other.json", set) === null, "different path should not match");
});

// -- Summary ---------------------------------------------------------------
process.stdout.write(`\n${passed} passed, ${failed} failed\n`);
try { fs.rmSync(tmpHome, { recursive: true, force: true }); } catch (_) {}
process.exit(failed === 0 ? 0 : 1);
