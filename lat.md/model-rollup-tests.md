---
lat:
  require-code-mention: true
---
# Model Rollup Backfill Test Specs

Model backfill tests pin resumable hourly folding, live-ingest handoff, pruned authority, and non-blocking maintenance admission.

## Empty Completion And Committed Progress

An empty database must commit `complete` before publishing its zero-of-zero terminal progress, without paying corpus-scale startup cost.

## Exact Resume And Pruned Authority

An interrupted hour-boundary pass must resume without gaps or duplicates while every `raw_pruned=1` row survives unchanged.

## Late Old-Hour Ingest Handoff

A source ingested into an hour older than the committed bookmark must stay exact because live ingest folds it directly and resume never erases it.

## Mid-Run Source Replacement Handoff

A source replaced between chunks must remain equal to the current raw group-by across already-bookmarked and future hours.

## Maintenance Admission Refusal

Rebuild admission must refuse both an active maintenance writer and a writer already queued behind the current backfill reader.

## Unexpected Failures Stay Generic

Unexpected SQL, invariant, or state errors must report a generic resumable failure while actual checkpoint terminals retain checkpoint-specific recovery copy.
