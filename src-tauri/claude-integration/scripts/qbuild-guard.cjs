#!/usr/bin/env node
"use strict";

const childProcess = require("child_process");
const fs = require("fs");
const path = require("path");

const DENY_REASON = "BLOCKED: qbuild is active — all file modifications must happen inside the worktree, not the original project directory. Use WORKTREE_PATH for all edits.";

function comparable(value) {
  const resolved = path.resolve(value);
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}

function contains(root, candidate) {
  const relative = path.relative(comparable(root), comparable(candidate));
  return relative === "" || (!path.isAbsolute(relative) && relative !== ".." && !relative.startsWith(`..${path.sep}`));
}

function realpathWithMissingLeaf(target) {
  const missing = [];
  let cursor = path.resolve(target);
  while (!fs.existsSync(cursor)) {
    const parent = path.dirname(cursor);
    if (parent === cursor) throw new Error(`no existing ancestor for ${target}`);
    missing.unshift(path.basename(cursor));
    cursor = parent;
  }
  return path.resolve(fs.realpathSync.native(cursor), ...missing);
}

function mainRepositoryRoot(cwd, gitExecutable) {
  const result = childProcess.spawnSync(
    gitExecutable,
    ["-C", cwd, "rev-parse", "--git-common-dir"],
    { encoding: "utf8", timeout: 1500, windowsHide: true },
  );
  if (result.error || result.status !== 0) return null;
  const raw = result.stdout.trim();
  if (!raw) return null;
  const commonDirectory = path.isAbsolute(raw) ? raw : path.resolve(cwd, raw);
  return fs.realpathSync.native(path.dirname(fs.realpathSync.native(commonDirectory)));
}

function qbuildIsActive(root) {
  return fs.readdirSync(root).some((entry) => entry.startsWith(".qbuild-lock."));
}

function evaluate(input, gitExecutable) {
  const filePath = input?.tool_input?.file_path ?? input?.tool_input?.notebook_path;
  const cwd = input?.cwd;
  if (typeof filePath !== "string" || filePath.length === 0) return null;
  let root;
  try {
    if (typeof cwd !== "string" || !fs.statSync(cwd).isDirectory()) return null;
    root = mainRepositoryRoot(cwd, gitExecutable);
    if (!root || !qbuildIsActive(root)) return null;
  } catch (_) {
    return null;
  }
  const lexicalTarget = path.resolve(cwd, filePath);
  let canonicalTarget;
  try {
    canonicalTarget = realpathWithMissingLeaf(lexicalTarget);
  } catch (_) {
    return DENY_REASON;
  }
  return contains(root, lexicalTarget) || contains(root, canonicalTarget) ? DENY_REASON : null;
}

function main() {
  try {
    const input = JSON.parse(fs.readFileSync(0, "utf8") || "{}");
    const reason = evaluate(input, process.argv[2]);
    if (!reason) return;
    process.stdout.write(`${JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "deny",
        permissionDecisionReason: reason,
      },
    })}\n`);
  } catch (error) {
    if (process.env.QUILL_DEBUG) console.error("qbuild-guard: error:", error.message);
  }
}

if (require.main === module) main();

module.exports = { contains, evaluate, realpathWithMissingLeaf };
