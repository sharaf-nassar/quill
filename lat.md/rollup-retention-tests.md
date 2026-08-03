---
lat:
  require-code-mention: true
---
# Rollup Retention Test Specs

Retention rollup tests pin coverage refusal, authoritative promotion, bookmark clamping, and separation from daily transcript counters.

## Missing Coverage Refusal

Retention must reject a model prune before moving its watermark or deleting raw when any affected hourly group differs from the raw refold.

## Missing Runtime Coverage Refusal

Retention must reject a runtime prune before moving its watermark or deleting events when finalized hourly rows differ from the deterministic source refold.

## Fold Then Prune Authority

One committed prune must promote exact model and runtime rows, preserve model authority through rebuild, clamp runtime state, and keep runtime reads independent of daily event counters.
