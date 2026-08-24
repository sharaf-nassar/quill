---
title: A done result before prepare can make integration repair unreachable
date: 2026-08-23
last_updated: 2026-08-24
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

A fresh single-task rail run rebuilt the preserved resolution from current
`main`, produced a new verifiable result, and integrated `quill-j76f.2` as
`0d43fad0b101b24601513ad87b53fb4951bd4c2b`. Recovery did not reuse or mutate
the immutable result from the failed run.

## Existing-file hub variant

Run `run-20260823T215909.bbDS2t` proved the same failure does not require an
add/add conflict. Tasks `quill-j76f.3`, `.4`, and `.5` were launched together
because the overlap report classified their shared edits to
`src-tauri/src/lib.rs` as hub contention rather than conflict. Pairing task `.4`
integrated first as `aa496b75d156032bbff5c99dd1064114d72de492`.

The already-verified `.3` and `.5` branches then both failed `prepare` with
content conflicts in `src-tauri/src/lib.rs`; `.3` also conflicted in
`lat.md/backend.md`. Their worktrees remain preserved for fresh-run recovery.
Hub classification describes a known shared file, not proof that independently
landed hunks will rebase cleanly.

## Prevention

- Inspect every required check before calling rail `result`; a required-gate
  failure means `status: failed` even when the worker says done.
- Do not normalize unsupported worker statuses such as `completed` until checks
  and commit evidence independently satisfy acceptance.
- Serialize tasks that share a declared hub file unless an earlier task has
  integrated and the next worker branches from that result. Hub classification
  is not proof of rebase compatibility, whether the file is new or existing.
- Resolve known sequencing-only unused-file failures before recording success,
  either by landing the real importer first or by adding an explicit temporary
  entry that the importer task removes.
- Treat `result` as the irreversible boundary: after it, the verified branch
  must need no edits before `prepare`.
