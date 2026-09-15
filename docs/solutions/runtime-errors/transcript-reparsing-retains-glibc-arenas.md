---
title: Concurrent transcript reparsing retains gigabytes in glibc arenas
date: 2026-09-13
component: session search / transcript analytics / Linux allocator
tags: [memory, glibc, pi, transcript, indexing, concurrency]
problem_type: performance
---

## Problem

Netdata recorded Quill's application group at 62,285 MiB private memory,
then 33,300 MiB swap. The original process stopped at 22:26 local time.
Relaunch reproduced growth in the Rust backend; the webview stayed around
230 MiB. This was not simply the 25 GB SQLite database occupying page cache.

Tracked in `quill-r93f`. Application code was not changed during diagnosis.

## Root cause

Independent consumers repeatedly materialize the same large Pi transcript:

- The retained filesystem sweep calls `SessionIndex::sync_with_roots` and
  `extract_messages_from_jsonl` in `src-tauri/src/sessions.rs`.
- `/sessions/notify` calls `index_session_notify_payload` in
  `src-tauri/src/server.rs`. It extracts before acquiring the writer lock
  through `replace_session_docs_batch`, so the lock does not serialize parsing
  against the sweep.
- Live retained analytics runs `reconcile_live_transcript_source` through
  per-batch `spawn_blocking` work in `src-tauri/src/lib.rs`.

`src-tauri/pi-integration/quill.ts` sends notify at `turn_end`; filesystem
changes independently admit retained work. Coalescing exists within individual
queues, not across all three consumers.

`parse_pi_session_file` reads the entire file. Analytics' separate
`parse_transcript_analytics_source_bytes` materializes all JSONL values, then
clones each value into `parse_pi_session_records`. The 256 MiB raw analytics
read limit is not a bound on the expanded JSON trees or concurrent consumers.

Allocation stacks captured concurrent threads in all three paths. Both Search
paths allocated 244,147,123-byte file buffers, consistent with the uniquely
large active Pi transcript in the local inventory. Analytics stacks also
showed JSON parsing and cloning. No raw session JSONL contents were inspected.

Large temporary allocations are freed, but glibc retains their pages in
per-thread arenas. This 64-CPU host accumulated more than 100 arenas. Repeated
work on different pool threads spreads high-water allocations across arenas;
small surviving allocations can prevent whole heaps from shrinking.

## Measurements

Existing `gcc` compiled a temporary, executable-filtered preload sampler using
`malloc_info`. A second sampler captured at most 400 allocation stacks and
called `malloc_trim(0)` once. Diagnostics stayed outside application code.
Every reproduction used a transient systemd cgroup with 6 GiB MemoryHigh,
8 GiB MemoryMax, and 1 GiB MemorySwapMax.

- Default allocator, five-minute run: 131 arenas, 6.65 GiB system heap,
  5.91 GiB free inside that heap, backend RSS reaching 5.96 GiB, and about
  1 GiB swap. The cap caused heavy reclaim pressure and stalled work.
- `MALLOC_ARENA_MAX=2`, comparable live workload: reduced peak RSS to about
  4.61 GiB, but still retained roughly 3.6 GiB free heap between parses.
  This is partial mitigation, not a fix or a controlled throughput benchmark.
- One-shot trim, same process: backend RSS fell from 5.988 GiB at 22:54:35
  to 2.057 GiB at 22:54:40; swap fell from 0.950 to 0.036 GiB. The process
  continued without resetting application data. RAM approached 6 GiB again
  within a minute as parsing continued.

Heap free-byte counters describe allocator ownership, not physical residency.
The same-process RSS/swap drop proves that substantial resident memory was
reclaimable allocator retention. The original 61 GiB process was already gone;
its exact allocation stacks cannot be reconstructed from this reproduction.

Evidence is local at `~/.cache/quill/diagnostics/quill-heap-20260913/`, including
XML heap snapshots, decoded allocation stacks, process traces, and journals.

## Misleading evidence

Tantivy's `couldn't find segment in SegmentManager` warning comes from stale
background merge completion. A successful Quill sync log occurs after commit.
The warning alone proves neither failed commits nor a leaked merge result.
The configured 50 MB writer heap also does not cap whole-transcript parsing or
merge allocations.

## Implementation (quill-r93f)

Canonical notify and sweep now share the retained-source owning boundary.
One parsed changed source supplies Search and analytics, with independent
committed fingerprints and failure recovery. Notify no longer performs its
separate Pi parse/two-table replacement. Pi payload session IDs must match the
persisted header before admission; valid pushed Search-only lineage/display
hints remain supported. A changed hint set is an explicit Search-only version
change; watcher admission without hints preserves the last applied set.

A persistent named standard thread owns JSON decoding. FIFO source admission
covers reading through normalized-output consumption, Search/SQLite commit,
and drop; whole-root orchestration yields that budget between source units.
Pi lines move directly into owned entries without cloning a whole JSON tree,
and raw bytes drop before extraction. Model preparation retains identities
rather than every source's observations or unchanged byte buffers; commit
reparses one source and checks fingerprint/identity before replacement.
Search now uses the analytics 256 MiB stable-read bound, preserves last-good
rows on failure, and cannot prune a newer live checkpoint using an older
inventory. Analytics caches evict expired keys on every access and cap typed
request variants at 64. No allocator caps or trimming were added.

The implementation follows official Tokio guidance to bound CPU-heavy
`spawn_blocking` work and use standard threads for persistent workers:
<https://docs.rs/tokio/1.52.1/tokio/task/fn.spawn_blocking.html>.
GNU's allocator tunable documentation confirms that arena count is not a heap
byte bound:
<https://sourceware.org/glibc/manual/latest/html_node/Memory-Allocation-Tunables.html>.
Research and the existing Tantivy 0.25 source do not justify changing merge
concurrency, which remains unchanged.

### Synthetic validation

An isolated real Search/SQLite test processes four growing versions of a
255,882,301-byte synthetic Pi transcript. With the default allocator, before
and after respectively take 359.8 and 349.8 seconds; final RSS is 524.3 versus
348.6 MiB. Peak RSS is **756.8 versus 836.3 MiB**: a roughly 10.5% regression,
still below the representative 2 GiB target. After RSS progresses
352.3 → 594.7 → 564.9 → 348.6 MiB, not linearly upward. Post-round allocator
allocated estimates stay about 6–11 MiB while hundreds of MiB can remain free
inside the heap. Unchanged admissions do not decode; changed ordinary live
versions decode once and deliver the expected complete documents.

This large-string fixture and serial baseline do not reproduce the incident's
many-small-object/concurrent-Tokio allocator pattern. They establish bounded
owning-path behavior and distinguish live estimates from residency, not a
numeric real-workload memory improvement. Focused tests additionally cover a
stalled real writer plus overlapping append, independent rollback/retry,
oversize last-good retention, cache eviction, and compact model-plan drift.
Exact specs/commands are in `lat.md/transcript-memory-tests.md`; raw synthetic
logs, baseline executable identity, recipe, and limitations are archived at
`~/.cache/quill/diagnostics/quill-r93f-implementation-20260914/`.

### Allocation-dense overlapping validation

A second fixture uses native user, assistant/tool-call, and tool-result groups,
with 128 small nested objects in both tool arguments and result details. Three
real owning paths overlap each round; fresh caller threads remain parked until
eight rounds finish, representing rotating workers whose arenas remain alive.
Four growing versions alternate with four identical versions. Every round
checks complete Search documents, events, tool actions, and model observations.

A scratch checkout of original HEAD `22311fc` contains only the identical
fixture/measurement helper, a test parse counter, and benchmark access to the
original notify owner. The complete like-for-like 4 MiB comparison uses the
same external 4 GiB MemoryMax, 3 GiB MemoryHigh, and zero-swap safety limits,
without allocator tuning or trimming:

| Measurement | Original | Fixed |
|---|---:|---:|
| Eight owning rounds | 17.130 s | 8.060 s |
| Final/peak RSS | 2178.6 MiB | 217.1 MiB |
| Retained caller threads | 24 | 24 |
| Final arenas at least 64 MiB | 16 | 1 |
| Final total arenas | 34 | 34 |
| Identical-version parsing | notify reparses | zero decodes |

Final allocator-live estimates are about 6–7 MiB in both runs. The original
replicates large free-heap high-water marks across 16 parsing callers; fixed
JSON decoding remains on one thread. Total arena count is **not** flat: normal
caller/SQLite allocations still create small arenas. These single synthetic
runs measure about 90% lower final RSS and 53% less owning time, not a promised
percentage improvement for the installed application.

The fixed 32 MiB dense run also completes all eight rounds: 24 retained
callers, one large arena throughout, 55.126 seconds owning time, 914.1 MiB
peak RSS and 906.6 MiB final RSS. An original-code 8 MiB attempt grew past
2.9 GiB RSS by round four, stalled under its safety limits, and timed out at
300 seconds. That run is **incomplete**, not a successful baseline or a speed
comparison; original 32 MiB scaling was deliberately not attempted. The input
cap and one-source admission do not promise a universal 2 GiB heap bound for
arbitrary JSON expansion.

The evidence archive now also contains the scratch adapter patch, binary
hashes, exact capped commands, per-round `malloc_info` XML, and complete logs.
The lifetime audit additionally moved Learning's existing redaction/compression
inside admission and pinned decoder self-nesting rejection. Context extraction
propagates read failures and uses saturating window arithmetic; its final
explicitly requested wire response transfers to the caller without truncation.
Response-byte/cache policy remains a separate risk, not a demonstrated cause
of this background incident.

## Final verification

Independent review found and corrected two descendant fan-out defects:
notification hints must stay with their source, and a descendant classification
error must record that descendant's failure while preserving last-good rows.
The new owning regression reproduced both failures before the fixes and passed
afterward. The reviewer rechecked the fixes with no remaining blockers.
Context requests intentionally share the source FIFO, so they may wait behind
earlier work; this latency tradeoff is explicit rather than bypassing admission.

Final-code validation passed 577 Rust tests and 64 Node tests, clippy with
warnings denied, formatting, typecheck, ESLint, knip, repository hooks, lat
checks, and the production-mode no-bundle build. Eleven Rust tests remain
ignored by default; the described measurements were run explicitly.

The rebuilt application also passed an isolated runtime qualification using
private HOME/data/transcript roots, private D-Bus/runtime directories, Xvfb,
and ports 19886/19887. Eight rounds sent 64 HTTP notifications over four growing
versions of a roughly 4 MiB synthetic Pi transcript, followed by a 120-second
recovery interval. Final results remained 254 Search documents, 508 events,
127 tool actions, and 127 model observations. Backend sampled RSS ranged from
280.1 to 346.4 MiB and ended at 345.9 MiB. Swap, memory throttling, OOM events,
and Quill warning/error logs were all zero. The external 4 GiB maximum and
3 GiB high-water safety limits never engaged; no allocator tuning or trim was
used. This is synthetic full-application evidence, not a production-corpus soak.

The first runtime harness attempt queried a nonexistent model-table column;
only the harness was corrected to use `source_session_id`, and qualification
was repeated in a fresh sandbox. Both temporary app services were stopped.
Final runtime traces, journal, harness, and post-review Rust gates are archived
under `quill-r93f-implementation-20260914/runtime-qualification/` beside the
other local evidence.

The installed AppImage was left unchanged by explicit user request. The
original `quill-protected.service` launch retains its external caps and
`MALLOC_ARENA_MAX=2`; ordinary desktop relaunch does not inherit them. The fix
is in the checkout and temporary build, not deployed to that installed binary.

## Review follow-up: checkpoint persistence and async waits

Rechecking the review of `39361c7` identified two concrete defects. A synthetic
32-source run rewrote `index_state.json` 32 times between source commits,
writing 147,412 bytes before the final batch flush. Keeping an old checkpoint
reader open also proved that `fs::write` truncated and rewrote that reader's
file instead of replacing it atomically. These are measured fixture results,
not estimates of production write volume or startup duration. After the fix,
the same 32-source fixture produced zero intermediate writes and one final
8,941-byte checkpoint, with all documents and checkpoints surviving reopen.
The open-reader regression also passed. Runtime comparison is not claimed;
this checks write count/volume and retained results.

Search now persists checkpoints at owning sweep, root-reconciliation, and live
drain boundaries, including earlier successes when a later source fails.
Individual Tantivy commits remain intact: changing their granularity would
change rollback isolation and live visibility, not merely reduce JSON writes.
Serialization streams through a buffered same-directory temporary file rather
than allocating a second whole-map String. File synchronization precedes atomic
replacement under the state mutex. Parent-directory synchronization is not
promised, so this does not claim complete power-loss durability.

The live drain keeps analytics results/events independent when a checkpoint
flush fails and retries Search. Legacy notify failures retain their pending
generation and reuse capped backoff. Older completions cannot remove newer
payloads. Search invalidation still fires on a possible partial commit, and
sweep analytics events precede Search-error propagation.

Learning digest construction now awaits `spawn_blocking`, retaining the source
budget through fetch, redaction, and compression. The current-thread Tokio
regression requires a sibling `join!` future to release the blocked digest
worker. Fresh Pi-header admission validation also runs in the blocking pool;
it is not replaced with a cached source-key comparison.

Several earlier review recommendations were rejected after tracing ownership:

- Legacy checkpoints lacked proof of a successful replacement: the old sweep
  could log extraction/indexing errors and still record a fingerprint.
  Treating `canonical_path: None` as automatically current could permanently
  preserve incomplete documents. One-time source revalidation remains intact.
- Pi keys do encode host/header identity, but a cached key does not revalidate
  a file replaced after discovery. Fresh header validation stays.
- Search errors cannot escape before the independent analytics commit.
  Combined job retry is intentional: successful consumers skip unchanged work,
  while the failed consumer keeps its own pending hints.
- The fallback identity check can observe a file that changed and then reverted.
  It is not dead code. The legacy drain can also recover a source that became
  valid after initial request validation.
- Global source FIFO latency, bounded model-plan reparsing, and scoped test
  helpers are explicit tradeoffs, not demonstrated correctness defects.

Final follow-up validation passed 580 Rust tests with 11 ignored, 64 Node
tests, formatting, clippy with warnings denied, typecheck, ESLint, knip,
repository hooks, backend build, and LAT checks. Independent read-only
re-review found no remaining blockers. Validation used synthetic data;
installed application and production state were not modified.

References checked for this follow-up:

- [Tantivy 0.25 IndexWriter](https://docs.rs/tantivy/0.25.0/tantivy/indexer/struct.IndexWriter.html)
  defines commit durability and rollback scope.
- [Tokio block_in_place](https://docs.rs/tokio/1.52.1/tokio/task/fn.block_in_place.html)
  warns that sibling `join!` branches still suspend and recommends
  `spawn_blocking` for that case.
- [tempfile NamedTempFile::persist](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist)
  documents atomic replacement but no implicit file/directory synchronization.
