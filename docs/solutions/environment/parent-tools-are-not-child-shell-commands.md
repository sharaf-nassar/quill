---
title: Parent tools are not child shell commands
date: 2026-08-24
last_updated: 2026-08-24
component: implement-ready / pi-subagents
tags: [subagents, tools, lat, orchestration, retries]
problem_type: workflow
---

# Parent tools are not child shell commands

## Problem

Attempt 1 for `quill-3cmg.7` stopped before editing because the worker tried to
execute the parent tool name as a shell command:

```text
lat_search: /bin/bash: line 1: lat_search: command not found
```

The task prompt repeated the requirement to run `lat_search`, but the builtin
worker did not have that parent extension tool. The clean worktree proved this
was a prompt/capability mismatch, not a missing project dependency.

## Root cause

Pi tool names and shell commands are different interfaces. A tool available to
the orchestrator is not automatically exposed to a fresh worker, and its name
does not become an executable in `PATH`. The repository does ship the `lat`
CLI, whose shell syntax is `lat search` and `lat section`, but that is separate
from the parent's `lat_search` and `lat_section` tools.

## What didn't work

- Repeating a parent-tool instruction verbatim in a worker prompt gave the
  worker an impossible requirement.
- Treating `command not found` as a reason to install software would have been
  wrong. No package was missing; the requested interface existed only in the
  parent session.
- Relaunching the same prompt would have repeated the stable failure signature.

## Fix

The orchestrator recorded the failed result, passed retry-gate, and changed the
attempt-2 prompt to state that parent-side LAT search was already complete. It
also named the installed CLI syntax for any extra local lookup and explicitly
forbade executing `lat_search` in the shell.

The retry completed as bead `quill-3cmg.7` and squash commit
`ff11b5c48ff8fa712b6f1001e3a3d0ef72c3d59e`.

## Prevention

- Run required parent-only discovery before dispatch and pass workers the
  resolved files, sections, and decisions they need.
- If a worker must use a repository CLI, name and verify its real executable
  syntax instead of reusing a parent tool name.
- Classify this failure as a task-prompt defect. Amend the prompt and use the
  retry gate; do not ask the human to install a nonexistent shell wrapper.
