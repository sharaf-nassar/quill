---
lat:
  require-code-mention: true
---
# Widget View Tests

These tests protect persistent widget view controls without browser-test infrastructure.

## Stored Range Preference

Only 1H, 6H, 24H, and 7D restore from local storage. Missing, invalid, inaccessible, or unwritable storage degrades to the 1H default without breaking current-session selection.
