---
lat:
  require-code-mention: true
---
# Model Rollup Backfill Test Specs

Model rollup tests pin resumable hourly folding, read-time invalidation, frozen-corpus equality, pruned authority, and maintenance admission.

## Empty Completion And Committed Progress

An empty database must commit `complete` before publishing its zero-of-zero terminal progress, without paying corpus-scale startup cost.

## Exact Resume And Pruned Authority

An interrupted hour-boundary pass must resume without gaps or duplicates while every `raw_pruned=1` row survives unchanged.

## Backfill Transaction Abort And Exact Resume

A failure after a chunk fold but before bookmark persistence must roll back both halves, preserve the committed prefix, and resume to exact raw-refold equality.

## Late Old-Hour Ingest Handoff

A source ingested into an hour older than the committed bookmark must stay exact because live ingest folds it directly and resume never erases it.

## Mid-Run Source Replacement Handoff

A source replaced between chunks must remain equal to the current raw group-by across already-bookmarked and future hours.

## Source Delete And Authoritative Re-ingest

Re-ingest must never add retained raw into an authoritative pruned bucket, and explicit source deletion must remove both raw evidence and every hourly authority for that source.

## Live Suppression Read Time Invalidation

Suppression flips must hide and restore completed overview and history results immediately without rewriting hourly rows or collapsing NULL token evidence into zero.

## Frozen Corpus Raw Hybrid Equality

A bounded 90d fixture derived from an immutable corpus must produce byte-exact normalized overview and history outputs for 24h, 30d, and 90d raw versus hybrid reads.

## Maintenance Admission Refusal

Rebuild admission must refuse both an active maintenance writer and a writer already queued behind the current backfill reader.

## Unexpected Failures Stay Generic

Unexpected SQL, invariant, or state errors must report a generic resumable failure while actual checkpoint terminals retain checkpoint-specific recovery copy.
