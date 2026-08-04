---
lat:
  require-code-mention: true
---
# Runtime Rollup Test Specs

Runtime rollup tests pin deterministic closed-turn folding, source replacement atomicity, and ingest overhead.

## Logical Turn Finalization And Source Refold

Persisted event gaps must produce stable closed turns and exact source replacement folds without leaving partial raw, rollup, state, or generation changes.

The fixture covers 5-minute continuity, ordinary idle closure, tool waits below and above 6 hours, start-hour attribution, the finalized bookmark, the open turn, exact re-ingest, and rollback after a late registry failure.

## Runtime Source Delete Invalidation

Explicit source deletion must atomically remove runtime raw events, hourly authority, open-turn state, and retained daily counters so no deleted source can remain visible.

## Runtime Fold Burst Budget

Runtime finalization and folding must add no more than 10% p95 latency to representative burst-shaped session-event replacement batches.

The ignored release benchmark times 25 replacements of one 6,000-event batch against the identical production transaction with only runtime folding disabled; parsing, setup, and warm-up remain outside timing.

## Runtime Backfill Empty Completion

An empty runtime source set must complete in one terminal shared-runner chunk and leave no misleading bookmark.

## Runtime Backfill Chunk Resume

A source larger than the row target must prepare outside the permit, commit compact state within the transaction deadline, resume by source-end bookmark, and preserve pruned authority exactly.

## Runtime Backfill Live Replacement Handoff

A source replaced between chunks must remain exact even when SQLite reuses rowids, with the resumed pass recognizing rather than duplicating the live refold.

## Nonmonotonic Runtime Source Preparation

Backfill must sort a source's persisted events before folding because its contiguous rowid block does not guarantee timestamp order.

## Prepared Runtime Source Revalidation

A source changed after off-permit preparation must fail revalidation and roll back every staged reconciliation write and bookmark change.

## Hybrid Runtime Read And Indexed Open Tail

Completed reads must match the time-invariant raw reference while seeking each active source's open tail through the provider-and-chain-leading timestamp index.
