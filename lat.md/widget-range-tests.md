---
lat:
  require-code-mention: true
---
# Widget Range Query Tests

These tests protect exact frontend query windows and conditional breakdown reads.

## Internal Comparison Ranges Are Exact

Every internal comparison range resolves to exactly twice its displayed widget range and shares the same pinned lower-bound helper used by history readers.

No widget surface requests one today, so this guards the range table itself rather than a live caller.

## Displayed Windows Bound Every Query

Every logged widget query stays at or below its displayed range, code insights included.

## Breakdown Transitions Issue Unique Reads

Switching breakdown modes keeps one project request, scopes Skills to the selected range, and preserves stable command-and-argument cache identities.
