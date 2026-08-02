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
