---
title: Absolute hooksPath leaks Beads hooks into linked worktrees
date: 2026-08-19
last_updated: 2026-08-19
component: implement-ready
tags: [git, worktrees, hooks, beads, rail]
problem_type: environment
---

# Absolute hooksPath leaks Beads hooks into linked worktrees

## Problem

This repository configures an absolute hook directory at `.git/config:6`:

```text
hooksPath = /home/mamba/work/quill/.beads/hooks
```

Absolute `core.hooksPath` applies to linked worktrees too. A normal worktree
commit therefore reaches `.beads/hooks/pre-commit:15-20`, which enables
`BD_GIT_HOOK` and runs `bd hooks run pre-commit` against the shared board.
Task worktrees must not mutate Beads state.

## Root cause

Git resolves the absolute hook path independently of the worktree checkout.
Creating a linked worktree does not isolate repository hooks, while the Beads
hook intentionally performs board synchronization before the ordinary
pre-commit framework.

## What didn't work

- Assuming linked worktrees inherit only their checkout's files leaves the
  shared hook active.
- Treating the hazard as a reason to stop the run is unnecessary when the rail
  owns worktree creation and can override hooks per worktree.

## Fix

Use only rail-created task worktrees. The rail assigns each one a per-worktree
`core.hooksPath` pointing to an empty directory while leaving primary-checkout
hooks enabled. Run explicit validation in workers, then rely on integration
gates and primary-checkout hooks for enforcement.

Run `run-20260819T094748.NDhRQT` confirmed this path: bead `quill-r3re.4`
integrated as `0b541b8ea1f113c46862bc49b26b969987065616`, and review follow-up
`quill-r3re.5` integrated as `477842d00f1d034625c222252c1a386ee304f93c`,
without task-worktree hooks touching `.beads`.

## Prevention

- Create implementation worktrees through the rail, never plain
  `git worktree add`, while this absolute hook is configured.
- Workers must run format, lint, type, and test gates explicitly because their
  hooks are intentionally disabled.
- Keep Beads mutations and final commit hooks on the primary checkout under the
  orchestrator's actor.
