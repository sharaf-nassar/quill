---
lat:
  require-code-mention: true
---
# Runtime Rollup Test Specs

Runtime rollup tests pin deterministic closed-turn folding, source replacement atomicity, and ingest overhead.

## Logical Turn Finalization And Source Refold

Persisted event gaps must produce stable closed turns and exact source replacement folds without leaving partial raw, rollup, state, or generation changes.

The fixture covers 5-minute continuity, ordinary idle closure, tool waits below and above 6 hours, start-hour attribution, the finalized bookmark, the open turn, exact re-ingest, and rollback after a late registry failure.

## Runtime Fold Burst Budget

Runtime finalization and folding must add no more than 10% p95 latency to representative burst-shaped session-event replacement batches.

The ignored release benchmark times 25 replacements of one 6,000-event batch against the identical production transaction with only runtime folding disabled; parsing, setup, and warm-up remain outside timing.

## Source-less Runtime Union Burst Budget

The completed-rollup read with a source-less live branch must stay within 10% p95 of the raw runtime read for the same 6,000-event burst.

## Source-less Runtime Union

Completed runtime reads combine remote source-less events with active Pi live-source tails. Sessions folds that active Pi tail into live per-session runtime; sealing moves the same totals into hourly authority without duplication.

## Pi Sequential Ingest Bound

One thousand production-shaped singleton Pi events must use linear SQLite work, leave at most one raw residual row for active reads, preserve exact open-turn runtime, and keep late replay exact.

## Source-less Pi Session Evidence

Source-less Pi rows do not claim per-session or agent runtime coverage; absent retained evidence remains unknown rather than a measured zero.

## Pi Runtime Turn Boundaries

Pi tool execution pairs preserve the six-hour tool-wait window without becoming response boundaries; completed long turns yield one response and interrupted turns yield none.

## Runtime Backfill Empty Completion

An empty runtime source set must complete in one terminal shared-runner chunk and leave no misleading bookmark.

## Runtime Backfill Chunk Resume

A source larger than the row target must prepare outside the permit, commit atomically by source, resume by source-end bookmark, and preserve pruned authority exactly.

## Runtime Backfill Atomic Deadline Overrun

A source whose compact commit outlasts the advisory deadline must finish atomically, publish its bookmark with the full fold, and release the permit before another source.

## Runtime Backfill Failed Startup Resume

A later startup run must resume a durable failed status from its committed source bookmark and publish complete exact totals without a manual rebuild.

## Runtime Backfill Live Replacement Handoff

A source replaced between chunks must remain exact even when SQLite reuses rowids, with the resumed pass recognizing rather than duplicating the live refold.

## Nonmonotonic Runtime Source Preparation

Backfill must sort a source's persisted events before folding because its contiguous rowid block does not guarantee timestamp order.

## Prepared Runtime Source Revalidation

A source changed after off-permit preparation must fail revalidation and roll back every staged reconciliation write and bookmark change.

## Hybrid Runtime Read And Indexed Open Tail

Completed reads must match the time-invariant raw reference while seeking each active source's open tail through the provider-and-chain-leading timestamp index.

## Observed Agent Runtime During Failed Backfill

A successfully reconciled open child's source-local runtime must publish when global historical backfill has failed, while failed sources, lifetime aggregates, and agents without a retained chain remain unknown.
