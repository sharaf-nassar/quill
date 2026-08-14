---
lat:
  require-code-mention: true
---
# Pi Model Usage Test Specs

Pi transcript analytics tests pin provider identity, tree totals, tolerant parsing, and replay safety.

## All Branch Usage

Every Pi assistant message contributes direct token dimensions to aggregate totals, with its upstream provider and model retained as one provider-qualified model id.

## Version And Diagnostic Tolerance

Pi v2 and v3 sessions parse through the shared tolerant parser, while malformed records and invalid model or token fields produce bounded diagnostics instead of aborting later usage.

## Native Session Identity

Pi model-source identity comes from the session header id and cwd, independent of filenames and tree entry content.

## Idempotent All Versus Active Totals

Replacing one Pi source twice keeps one row per header-id and entry-id pair; overview totals include all branches while session totals include only the last-entry parent chain and say `active-branch`.
