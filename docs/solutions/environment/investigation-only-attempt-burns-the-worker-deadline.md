---
title: An investigation-only first attempt burns the whole worker deadline
date: 2026-09-11
last_updated: 2026-09-11
component: implement-ready / pi-subagents
tags: [subagents, timeout, retry-gate, resume, task-sizing, pi-integration]
problem_type: workflow
---

# An investigation-only first attempt burns the whole worker deadline

## Problem

Attempt 1 for `quill-3cmg.9` (exact-pair Pi span receipts) failed with:

```text
worker runtime: Subagent timed out after 1800000ms
```

The worktree was clean at the base SHA: 65 turns, 64 tool calls, all
`bash`/`read`, zero edits, no commit. The worker spent the full 30 minutes
reading the spec, five lat.md files, the Rust parser and fold paths, and
unpacking pi-coding-agent 0.84.0 and pi-agent-core from npm to verify
`message_update` payload shapes. Nothing was wrong with the code or the
task; the clock ran out before implementation began.

## Root cause

The bead legitimately required cross-repo verification (pin a minimum Pi
version against real event shapes) plus a six-file Rust/TS change. The
task prompt front-loaded the reading list with no time budget, and the
30-minute default deadline covers investigation and implementation
together. The prior learning
`repeated-worker-timeouts-require-bead-splitting.md` covers the case where
supervisor waits eat the clock; this is the sibling case where a broad
"read everything first" prompt does.

## What didn't work

- Treating the timeout as a sizing signal and splitting the bead. The
  worktree showed no partial implementation to split around; the work had
  not started.
- Relaunching fresh would have repeated the same reading pass and the same
  signature, which the retry gate correctly blocks.

## Fix

The orchestrator recorded the failed result, passed
`retry-gate --attempt 2`, and used `subagent({ action: "resume" })` on the
persisted child session with a 90-minute `timeoutMs` and a prompt stating
that investigation was complete and listing the implementation order. The
resumed worker kept its context, implemented, passed every gate, and
committed `eeec97d`, integrated as squash
`f7b3510f966b816374ef426562623faaf5313dde` for bead `quill-3cmg.9` in
roughly 30 minutes.

## Prevention

- When a bead needs external verification (npm pack, upstream source), say
  so in the prompt with an explicit time cap ("spend at most 10 minutes
  verifying shapes, then implement") and raise `timeoutMs` above the
  30-minute default at dispatch.
- On a clean-worktree timeout, resume the persisted session rather than
  relaunch: the reading is already in context, and resume with a new
  deadline is the concrete change the retry gate asks for.
- Ask the worker to commit a compiling partial and return `failed` with a
  real `error_signature` at a stated fraction of the deadline, so a timeout
  never again leaves zero evidence.
