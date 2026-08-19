---
lat:
  require-code-mention: true
---
# Pi Spool Retirement Test Specs

These tests pin one-way retirement of Quill-owned legacy Pi spool artifacts without importing them.

## Cutover sequencing

Retirement waits for persisted-source reconciliation; reporter reload and exact-generation acknowledgement are not prerequisites.

## Owned artifact cleanup

Retirement claims and removes only dead or already-claimed Quill spool files, preserves live and foreign files, records the no-import gap, and completes only after every owned writer exits.

## Symlink boundary

Retirement rejects a symlinked spool root and leaves every file in its external target untouched.
