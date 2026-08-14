---
lat:
  require-code-mention: true
---
# Pi End To End Validation

This measured test keeps Pi search admission, persisted usage totals, and bounded large-file folding within their feature budgets.

## Numeric Pipeline Evidence

An isolated post-registration transcript must become searchable within 15,000 ms, persist the exact all-branch usage sum, and fold a synthetic file of at least 100 MiB while reading no more than 1,048,576 tail bytes.

Two runs on 2026-08-14 passed. The slower successful timing was `{"searchable_ms":709,"usage_expected":236,"usage_actual":236,"large_file_bytes":104858143,"tail_bound_bytes":1048576,"tail_read_bytes":1048576,"tail_elapsed_ms":4}`.
