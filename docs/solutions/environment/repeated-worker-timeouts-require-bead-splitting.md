---
title: Repeated worker timeouts can hide supervisor-wait exhaustion
date: 2026-08-18
last_updated: 2026-08-18
component: implement-ready
tags: [beads, subagents, timeout, retry-gate, supervisor-decisions, task-sizing]
problem_type: workflow
---

# Repeated worker timeouts can hide supervisor-wait exhaustion

## Problem

Task `quill-oyie.4` failed twice with the same signature:

```text
worker runtime: Subagent timed out after 1800000ms
```

The second attempt left a 15-file dirty worktree with 2,323 insertions and 174
deletions, but no commit. The rail rejected attempt 3 because the signature did
not change. Evidence remains in run `run-20260818T020455.eIPi0x` and the
preserved `quill-oyie.4` worktree.

A later investigation found the patch was much closer to completion than the
timeouts suggested. `cargo test --manifest-path src-tauri/Cargo.toml pi_
--no-fail-fast` passed all 81 selected tests, clippy passed with warnings denied,
`lat check` passed, and `git diff --check` passed. Only `cargo fmt --check`
failed on one formatting change in `src-tauri/src/server.rs`.

## Root cause

The bead's declared file list omitted code paths required by its own acceptance
criteria. Current main drops validated Pi notify admission before the shared
queue at `src-tauri/src/server.rs:1515-1516`, starts legacy spool import
unconditionally at `src-tauri/src/server.rs:169`, and retention recognizes only
the old `live:pi:%` source shape at
`src-tauri/src/retention_engine.rs:1578`.

Both workers stopped for supervisor approval before changing those undeclared
files. Per this investigation's run timing, the fixed 30-minute child deadline
continued while each worker was detached for those decisions. The deadline then
expired before formatting and commit. The same timeout signature described the
clock, not a repeated code failure.

The original task was broad, but breadth alone was not the proven failure. The
preserved implementation compiles and passes its Pi tests. Incomplete ownership
metadata plus decision-wait deadline consumption made the work look less
complete than it was.

## What didn't work

- The first diagnosis treated the timeout as proof that the task was too large.
  Later compile and test evidence disproved that as the sole cause.
- A stronger-model retry kept the same incomplete file contract. It found two
  more required files, waited for approval, and hit the same wall-clock limit.
- Retrying again would not change the mechanism. The rail's unchanged-signature
  gate correctly stopped attempt 3.

## Fix

The recovery was split into three serial, test-bounded bugs:

- `quill-oyie.12` parses persisted Pi tracking/native entries into owned
  snapshots.
- `quill-oyie.13` adds the verified schema-45 backup, migration 46, atomic Pi
  ownership replacement, and canonical retention.
- `quill-oyie.14` wires Pi into the shared coordinator, watcher, startup,
  notify, and spool-import guard.

The chain is unlanded as of this writing. `quill-oyie.4` was superseded by
`quill-oyie.14`, which depends on `.13`, which depends on `.12`.

The preserved reference patch is:

```text
/home/mamba/.local/state/bd-orchestrate/quill-.run-20260818T020455.eIPi0x/investigation/quill-oyie.4-preserved.patch
```

Its SHA-256 is
`c2b9c182aabf2f5a94ca94cbfcc73e96c573a47f15db1a0d33a442d06cce2bc7`,
and `git apply --check` succeeds against current main. Each recovery worker must
use only the hunks owned by its bead rather than applying the whole patch.

## Prevention

- Make `Files:` include every path required by acceptance, especially server
  admission, migration markers, and retention consumers.
- Adjudicate likely file drift before dispatch when the spec names behavior in
  another subsystem.
- Do not infer task-size failure from a wall-clock timeout until the preserved
  patch has been formatted, compiled, and tested.
- When a worker waits for supervisor input, treat deadline consumption as part
  of the failure evidence. A runtime-level pause for decision waits belongs in
  pi-subagents, not in this repository.
- Split recovery work by a runnable test boundary, not by raw line count.
