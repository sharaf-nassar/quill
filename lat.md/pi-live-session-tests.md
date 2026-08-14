---
lat:
  require-code-mention: true
---
# Pi Live Session Test Specs

These tests pin the bounded Pi transcript fold and its honest Sessions presentation.

## Liveness

A valid Pi session file creates one provider-isolated live session from transcript evidence.

## Header Cwd

The live session takes cwd from the Pi session header rather than decoding its lossy directory name.

## Last Entry Timestamp

The newest complete tail entry supplies Pi activity time without consulting file mtime.

## Last Message Identity

The newest assistant message supplies its validated upstream provider and model without walking parent links.

## Deferred Initial Flush

An absent path remains a no-op until Pi's deferred first flush creates the transcript, which then folds normally.

## Equal Length Rewrite

An `(mtime_ns, len)` change cold-refolds Pi state even when a migration rewrite preserves file length.

## Idle Quiescence

A Pi transcript silent beyond the shared 15-minute cutoff releases its live session and file state.

## Explicit Token Gap

A live Pi row renders cumulative tokens as `—` while keeping retained turn evidence available.

## Ephemeral No Op

A Pi conversation with no session file produces no live row.

## Bounded Large Branch Tail

A synthetic branched Pi session of 104,857,951 bytes scans 1,048,576 bytes, the shared tail bound, from an isolated temporary file.

## Proven Live Lineage

One parent with two live declared children exposes exactly two linked sessions while an unlinked sibling stays independent and Pi native agent count stays unknown.
