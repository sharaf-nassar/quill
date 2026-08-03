---
lat:
  require-code-mention: true
---
# View Reader Contention Tests

These tests prove independent SQLite view readers preserve responsive analytics while ingest continues through the serialized writer.

## Five-Second Snapshot Allows Fast Queries And Ingest

A pinned five-second view snapshot must leave host and project queries at or below 100 ms p95 across 100 samples each, produce no lock or busy errors, and allow concurrent token ingest to persist.
