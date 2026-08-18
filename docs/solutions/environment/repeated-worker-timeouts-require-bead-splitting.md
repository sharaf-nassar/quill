---
title: Repeated worker timeouts require splitting the bead
date: 2026-08-18
component: implement-ready
tags: [beads, subagents, timeout, retry-gate, task-sizing]
problem_type: environment
---

# Repeated worker timeouts require splitting the bead

## Problem

Task `quill-oyie.4` combined a schema-45 backup and migration, persisted Pi
parsing, shared-coordinator wiring, atomic source replacement, startup recovery,
server notify routing, retention cleanup, tests, and lat.md updates. The approved
scope spans the reconciliation section in
`specs/028-pi-agent-tracking-hardening.md:1308-1341`.

Attempt 1 and the Fable escalation both hit the worker runtime limit with the
same signature:

```text
worker runtime: Subagent timed out after 1800000ms
```

The second attempt left a 15-file dirty worktree with 2,323 insertions and 174
deletions, but no commit. The rail rejected attempt 3 because the signature did
not change. Evidence remains in run `run-20260818T020455.eIPi0x` and the
preserved `quill-oyie.4` worktree.

## Root cause

The bead crossed several independently testable seams. A fixed 30-minute worker
slice could not inspect, implement, compile, test, document, and commit the full
change. Resuming the same broad scope on a stronger model changed the patch but
not the failure.

## What did not work

- A normal worker produced a large partial Rust patch, then timed out before
  tests and commit.
- A Fable finish-forward attempt started from that patch, added tests and docs,
  then timed out at the same limit.
- Another retry was not evidence-based. The rail's unchanged-signature gate
  correctly stopped it.

## Recovery

The run preserved the dirty worktree and filed `quill-oyie.11`, which blocks
`quill-oyie.4`. Refine that bug before dispatch. Split the work into bounded
slices such as migration and backup, parser and source identity, atomic storage
replacement, coordinator wiring, then documentation and qualification. Each
slice needs one focused command and a commit before the next starts.

No fix commit landed for `quill-oyie.4`; commits through
`7e1390d40ca62df1134567dad6d2558b82435994` cover only the completed predecessor
work.

## Prevention

- Split a bead before dispatch when it owns a migration, parser, coordinator,
  storage transaction, server wiring, and several test specifications at once.
- Treat a timeout with a large dirty diff as a sizing failure, not a request for
  a longer prompt.
- Preserve the worktree, record the stable signature, and let the retry gate
  stop identical failures.
- Give each recovery bead one compile or test boundary and one commit-sized
  outcome.
