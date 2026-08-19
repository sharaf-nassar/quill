---
title: Acceptance criteria written in the description block dispatch
date: 2026-08-19
last_updated: 2026-08-19
component: implement-ready
tags: [beads, rail, orchestration, acceptance, bead-authoring]
problem_type: workflow
---

# Acceptance criteria written in the description block dispatch

## Problem

`quill-dbs1` was a fully specified P2 bug — root cause with a file:line,
a repro query, a fix approach, a declared file list, and concrete
acceptance criteria naming the failing-first test. The rail's first
survey still put it in `unacceptable`, not `ready`, and `claim` would
have refused it.

The prescribed remedy for that bucket ("no acceptance criteria, refine
before dispatch — run `/file <id>`") is wrong for this bead shape, and
following it re-authors a specification that already exists.

## Root cause

The criteria were prose inside `description`, not the structured
`acceptance_criteria` field. `bd show --json` omits the field entirely
when empty, so the bead's JSON simply had no `acceptance_criteria` key
while its description text contained a full `Acceptance criteria:`
paragraph.

The rail gates on the structured field only
(`~/.beads/rail/implement-ready.sh:306`):

```jq
def acceptable: ((.acceptance_criteria // "") | gsub("\\s";"") | length) > 0;
```

The asymmetry is what makes this hard to see: the very next definition
in the same jq program parses `Files:` *out of the description prose*
for the overlap guard (`implement-ready.sh:308-316`, and again in
`task_files`). So one declaration reads from prose and the adjacent one
does not, and a bead that declares both in prose is half-visible to the
rail — files guarded, criteria invisible.

Beads created by `/spec` carry the structured fields; a bead created
with everything crammed into `--description` does not. Both render
identically to a human reading `bd show`.

## Fix

Check whether the criteria exist as prose before treating the bucket as
a refinement request. When they do, promote the existing text verbatim —
this is a copy, not authoring, so it does not violate the rule against
an orchestrator inventing acceptance criteria:

```bash
bd update <id> --acceptance "<exact text lifted from the description>"
```

The flag is `--acceptance`, not `--acceptance-criteria`. No `--files`
flag exists or is needed, because the rail derives that from the
description's `Files:` line.

For `quill-dbs1` the promotion alone moved it from `unacceptable` to
`ready` with all four declared files parsed, and the run integrated as
`844ceaf` with no further bead edits.

## Prevention

Treat `unacceptable` as "the structured field is empty", not "nobody
wrote criteria" — they are different states with different remedies,
and only the first is visible in the survey output. When filing a bead
by hand, put criteria in `--acceptance` even if the description repeats
them; prose alone reaches a human but never reaches the gate.
