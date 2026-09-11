---
title: Worker edit tool resolves relative paths against the primary checkout
date: 2026-09-11
last_updated: 2026-09-11
component: implement-ready / pi-subagents
tags: [subagents, worktree, edit-tool, cwd, orchestration, rail]
problem_type: environment
---

# Worker edit tool resolves relative paths against the primary checkout

## Problem

Attempt 1 for `quill-1o03` (run `run-20260911T013131.2ATZAr`) was told to
work only inside its rail worktree and did `cd` there in every `bash` call.
Its first three `edit` calls used repository-relative paths:

```text
src-tauri/pi-integration/quill.ts
src-tauri/pi-integration/quill.test.mjs
src-tauri/src/models.rs
```

Those edits landed in `/home/mamba/work/quill` (the primary checkout on
`main`), not in
`.worktrees/implement-ready/run-20260911T013131.2ATZAr/quill-1o03`. The
worker noticed, moved the diffs into the worktree by hand, and restored
main. Had it not, the orchestrator's next `prepare` would have run with a
dirty primary index and the rail's "never edit while a prepared squash
exists" rule would have been violated silently.

## Root cause

A `bash` tool call's `cd` changes only that one shell process. The `edit`,
`read`, and `write` tools resolve relative paths against the subagent
session's cwd, which is the primary checkout the orchestrator launched
from, not the worktree the task prompt names. "Work only in the worktree"
therefore constrains shell commands but not file tools unless every file
tool path is absolute.

## What didn't work

- Repeating "cd into the worktree first" in the prompt. It is honoured by
  `bash` and irrelevant to `edit`.
- Trusting the worker's self-report alone. The orchestrator verified with
  `git status --short` on the primary checkout before `prepare`; that check
  is what makes the recovery trustworthy.

## Fix

The worker relocated the three diffs into the worktree and reset the primary
files, then committed `182e0a2` on the task branch, integrated as squash
`008f2db9f5e9898a98c48cc6c3670570e407f372` for bead `quill-1o03`.

## Prevention

- Task prompts must state: every `edit`/`read`/`write` path is absolute,
  prefixed with the worktree path. Relative paths hit the primary checkout.
- The orchestrator checks `git status --short` on the primary checkout
  before every `prepare`; any unexpected change there is a worker leak, not
  run state, and blocks integration until attributed.
- Prefer launching workers with `cwd` set to the worktree when the spawn
  path supports it, so relative file-tool paths and shell paths agree.
