---
lat:
  require-code-mention: true
---
# Rollup Backfill Concurrency Test Specs

These tests prove concrete model and runtime backfills remain exact and WAL-bounded when maintenance defers both backfill and live ingest.

## Model Quiesce Ingest And WAL Bound

A committed model prefix must remain unchanged during quiesce, then resume after deferred old-hour ingest without loss or duplication; every real chunk WAL stays within its configured row estimate and truncates to zero.

## Runtime Quiesce Ingest And WAL Bound

A committed runtime source prefix must remain unchanged during quiesce, then resume after deferred source replacement without loss or double fold; every real chunk WAL stays within its configured row estimate and truncates to zero.
