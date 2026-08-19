---
title: JSON-only worker results can fail checked acceptance after success
date: 2026-08-19
last_updated: 2026-08-19
component: implement-ready
tags: [subagents, acceptance, worker-output, rail, orchestration]
problem_type: workflow
---

# JSON-only worker results can fail checked acceptance after success

## Problem

The escalated `quill-r3re.4` worker completed its patch, committed it, and
returned the rail's required JSON result. Pi-subagents still marked the child
failed with:

```text
Acceptance rejected: Structured acceptance report not found.
```

The run state records `state: "complete"` at
`/tmp/pi-subagents-uid-1000/async-subagent-runs/09439f79-b2ee-45dc-8ddf-317b02156bb1/status.json:7`
and the canonical worker result, including commit
`15b410750726bc117053e947b5069e8cc12b665d`, at the same file's line 189.
The acceptance rejection begins at line 195.

## Root cause

The worker prompt required one plain JSON object for the rail. The parent launch
also requested pi-subagents checked acceptance, whose runtime expected its own
structured acceptance-report format. Adding an `acceptance_report` property to
the JSON object did not satisfy that separate parser, so successful execution
and acceptance-envelope parsing produced contradictory statuses.

## What didn't work

- Waiting for another worker message could not change the completed child.
- Steering failed after the foreground route had already disappeared.
- Treating the wrapper status as the task result would have discarded a clean,
  fully tested commit.

## Fix

Use the saved worker JSON as the rail result, then let `verify-worker` prove the
canonical commit, clean worktree, branch ancestry, and changed files. That path
verified the worker commit and integrated bead `quill-r3re.4` as squash commit
`0b541b8ea1f113c46862bc49b26b969987065616`.

For future rail workers, do not combine a strict JSON-only return contract with
a generic checked-acceptance parser unless both expect the same envelope. Omit
the extra acceptance policy or request its exact supported report syntax.

## Prevention

- Treat child runtime acceptance and rail verification as separate gates.
- Preserve and inspect the saved worker output before classifying a wrapper
  rejection as an implementation failure.
- Keep the rail's result schema stable; adapt the subagent launch contract
  around it rather than making workers emit two incompatible final formats.
