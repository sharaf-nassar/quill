---
title: A done result before prepare can make integration repair unreachable
date: 2026-08-23
last_updated: 2026-08-23
component: implement-ready
tags: [beads, rail, integration, retry-gate, worktrees, knip]
problem_type: workflow
---

# A done result before prepare can make integration repair unreachable

## Problem

Run `run-20260823T195123.rtBvWa` recorded task `quill-j76f.2` as done after
worker commit `41e6308851a755835a093fc04f9de77f66579d43`. Its checks still named
one failed required gate:

```text
npm run knip: reports only src/web/httpTransport.ts unused until prerequisite web entry lands
```

The repository specification says every P6 gate must be green at
`specs/029-web-ui-server.md:757-759`. The result should therefore have been
recorded failed, not normalized to done.

`prepare` then rebased the worker after scaffold bead `quill-j76f.1`, squash
commit `afcd373dcbfd673484aa5b6a187ee5028f991eea`, and hit an add/add conflict in
`src-tauri/src/web_server/mod.rs`. The shared-file sequence is explicit at
`specs/029-web-ui-server.md:782-799`: the scaffold creates the module and the
protocol task extends the same contract surface.

A concrete resolution was built and verified in the preserved worktree:

```text
/home/mamba/work/quill/.worktrees/implement-ready/run-20260823T195123.rtBvWa/quill-j76f.2
```

Its `134796a` head combines the router scaffold and protocol contract, registers
the standalone TypeScript transport as a temporary Knip entry, and passes
TypeScript, Node, Rust, Knip, and LAT gates.

## Root cause

Rail result artifacts are immutable. Once attempt 1 was recorded done, a changed
branch could not be re-recorded or re-verified:

```text
implement-ready: refusing to overwrite artifact: .../attempts/quill-j76f.2/1/result.json
implement-ready: worker SHA is not worktree HEAD
implement-ready: task branch moved after worker verification
```

Attempt 2 was also unavailable because the hard retry gate correctly rejected a
retry after a non-failed result:

```text
attempt 1 did not fail; nothing to retry
```

The integration repair was concrete, but the earlier result classification had
removed every rail-compliant path to land it.

## What didn't work

- Treating a worker's `status: done` as authoritative despite a failed required
  check made the result artifact contradict acceptance.
- Resolving the rebase and adding the missing Knip entry after `verify-worker`
  moved the branch, so the rail correctly refused preparation.
- Re-running `result` could not amend attempt 1 because result artifacts are
  write-once.
- `retry-gate` could not be overridden: its denial was hard because attempt 1
  was recorded successful.

## Fix

No fix commit landed in this run. Task `quill-j76f.2` remains open with the
resolved worktree preserved and downstream web-server tasks stranded. Its bead
notes contain the exact recovery path and gate outputs.

A future recovery must start with a rail-verifiable branch whose final worker
result includes the scaffold merge and a green Knip gate. Do not reuse the done
attempt artifact as proof for a different branch head.

## Prevention

- Inspect every required check before calling rail `result`; a required-gate
  failure means `status: failed` even when the worker says done.
- Do not normalize unsupported worker statuses such as `completed` until checks
  and commit evidence independently satisfy acceptance.
- When the first task creates a file that another ready task also declares,
  serialize them despite a hub-contention classification; add/add conflicts are
  not ordinary disjoint hub edits.
- Resolve known sequencing-only unused-file failures before recording success,
  either by landing the real importer first or by adding an explicit temporary
  entry that the importer task removes.
- Treat `result` as the irreversible boundary: after it, the verified branch
  must need no edits before `prepare`.
