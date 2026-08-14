---
lat:
  require-code-mention: true
---
# Widget Range Query Tests

These tests protect exact frontend query windows and conditional breakdown reads.

## Internal Comparison Ranges Are Exact

Every internal comparison range resolves to exactly twice its displayed widget range and shares the same pinned lower-bound helper used by history readers.

## Displayed Windows Bound Every Query

Code insights may request exactly two displayed periods; every other logged widget query stays at or below its displayed range.

## Breakdown Transitions Issue Unique Reads

Switching breakdown modes keeps one project request, scopes Skills to the selected range, and preserves stable command-and-argument cache identities.
