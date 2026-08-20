---
title: Live agent rails show roles instead of models
date: 2026-08-24
component: live tracker / transcript watcher
tags: [live-tracker, transcript-watcher, re-ingest, pi, agent-rail, starvation]
problem_type: bug
---

## Problem

Live sub-agent rails in the Sessions breakdown label agents with their role
(`reviewer`, `worker`) instead of their model family (`Sol`, `Sonnet`). It
reads as a frontend regression in the chip label. It is not — the label code
is correct and unchanged.

## Root cause

The role is the *fallback* the chip shows when `model_id` is null
(`src/utils/format.ts:66-70`), specified at
`lat.md/live-subagent-count-tests.md:61`. A Pi agent's `model_id` has exactly
one source: the live fold of the child's own transcript
(`src-tauri/src/live_tracker.rs:1368`, fed by `pi_model` at `:1764`). Seeing a
role therefore means the fold never ran, not that the label logic changed.

The fold had stopped running because the transcript watcher is a single
thread that also runs the retained-side scan inline:
`admit_pending` folds the batch (`src-tauri/src/transcript_watcher.rs:352`)
and then calls `sync_search_index` synchronously (`:354-356`). While that
scan runs, the loop never reaches `rx.recv_timeout`, so no watcher event is
collected and the unconditional 120-second `sweep_live_tracker` backstop
(`:300`) never fires.

What made that scan permanent: re-ingest flags clear only when every file
succeeds (`src-tauri/src/sessions.rs:1891-1936`), and any set flag makes
`force_reingest` clear the whole mtime cache each pass (`:1753-1757`). One
Codex rollout that can never resolve its provider-native identity keeps
`failures=1` forever, so the corpus is fully re-extracted every ~12 minutes
in a self-sustaining loop.

Reporter pushes arrive on the HTTP thread and are unaffected, which is why
the agent still appears with a correct role, lineage, and runtime. That
asymmetry is the trap: the row looks healthy, so the missing model reads as a
display bug rather than a dead fold. Claude and Codex have no push path at
all, so during the same window their sessions silently go stale.

## What didn't work

- **Reading the label code and its history.** `git log -S` on the fallback in
  `src/utils/format.ts` shows it unchanged since `445f6a3` (bead
  `quill-eeo`), and the file has not been touched since `2f3bf78`. Correct
  conclusion, zero progress — the bug was two subsystems away.
- **Replaying the real transcripts through the tracker.** A throwaway
  worktree test that copied the actual parent and child session files into a
  fixture layout resolved `model_id: Some("gpt-5.6-sol")` correctly, both
  file-only and with the reporter push applied first. This *disproved* the
  backend-logic hypothesis and was the turn that redirected the whole
  investigation toward runtime state. Worth doing early.
- **Guessing at eviction, tombstones, and lineage.** The `ended` tombstone map
  added in the same week looked like an obvious culprit for "transcript
  refuses to fold". It was not involved; a live child never carries a
  tombstone. Reading a plausible recent diff is not evidence.

## Fix

Filed as two beads, unlanded as of this writing:

- `quill-fqwp` (P0) — separate terminal from retryable extraction failures so
  a permanently-unparseable transcript cannot hold the re-ingest flags set.
- `quill-ppbv` (P1) — move `sync_search_index` and `reconcile_all` off the
  watcher thread so no long retained-side scan can starve live folding again.

`quill-w5bu` (P3) separately decides whether the chip may show a role at all;
it is deliberately blocked on the two above, because the fallback only looks
wrong while model evidence is arriving minutes late.

## Prevention

When live-derived UI shows a fallback value, ask which producer feeds that
field before reading the consumer. Push-fed and fold-fed fields land on the
same row from different threads, so a row can be simultaneously fresh and
starved, and the fields that are wrong tell you which producer died.

The app logs are the fastest evidence in this class — a repeating sweep line
with no idle gap between passes is the whole diagnosis:

```
Codex identity re-ingest sweep: files=6484 failures=1 ... duration_ms=730750
```

Reporter receipt rows corroborate saturation: two events minutes apart
sharing one `accepted_at_ms` means the durable writer is far behind, not that
the events arrived together.

Structurally: a retry that clears its own cache on failure is a loop unless
some failure class is terminal, and long retained-side work on a thread that
also owns live state is a starvation bug waiting for a slow corpus.
