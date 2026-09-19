---
title: Retained transcript failures need separate rejection checkpoints
date: 2026-09-19
component: retained ingestion / Pi parser / Session Search
tags: [pi, codex, streaming, ingestion, retry, memory]
problem_type: bug
---

## Problem

Historical Pi self-resume metadata, large Pi transcripts, and empty Codex
rollouts repeatedly failed retained ingestion. Retrying unchanged bytes
could keep reingest markers armed and spend scan time on deterministic
failures instead of new evidence.

## Root cause

The old Pi producer could persist a resume whose previous session was its
own session. The producer guard fixed new records but did not repair
historical ones. Retained Pi reads also inherited a 256 MiB whole-file cap.
Raising that cap would recreate the raw-buffer and JSON ownership pressure
described in `transcript-reparsing-retains-glibc-arenas.md`.

A failed-source flag is insufficient: I/O and storage failures need retries,
while stable invalid content needs a negative checkpoint. Successful source
fingerprints cannot safely double as failed-attempt fingerprints. An empty
Codex rollout has no native session identity, not a successful empty session.

## Fix

`quill-2ozu` streams Pi physical JSONL records on the existing fixed decoder
thread, hashing original bytes and verifying open-file and path stability.
It retains searchable text and existing bounded tool previews, not unindexed
image payloads or thinking bodies. Limits are explicit: 4 GiB input,
256 MiB per record, 100000 records, and 256 MiB serialized retained evidence.
They bound representation, not exact allocator RSS. Ignored records do not
consume the retained-evidence budget. Over-budget sources reject atomically
rather than silently truncating conversation text.

Only persisted resume events with an equal previous/current session id get
the obsolete previous id removed before strict validation. Live protocol
validation remains unchanged, and source transcripts are never rewritten.

Schema 49 and the Search sidecar store rejection metadata separately from
last-good checkpoints: canonical path, attempted stat fingerprint, parser
policy version, and bounded diagnostic. Stable permanent failures settle
unchanged work. Changed sources and parser-policy increments rearm it.
Transient I/O, graph, drift, and commit failures remain retryable. A complete
inventory with settled rejections can clear a reingest marker, but rejection
is not successful ingestion or proof permitting destructive root pruning.

## Verification

Regression recipes and assertions live in
`lat.md/pi-session-parser-tests.md` and `lat.md/transcript-memory-tests.md`.
The ignored 400 MiB tool-output qualification checks Search, usage, original
hashes, repeated versions, and process peak memory. The existing 244 MiB
text-heavy qualification checks compatibility with previously supported
sources. Run memory qualifications separately in fresh processes, using
synthetic data rather than migrating the user's active database.

## Prevention

Carry permanent-versus-transient classification through root commit paths;
flattening a parser error to a string can accidentally restart the loop.
Do not classify every failed commit as bad source content. Keep migration
rewind fixtures current, and retain existing metadata when adding a new
checkpoint column. Test an empty producer file through its first valid
append and through process restart.
