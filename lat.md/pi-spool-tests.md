---
lat:
  require-code-mention: true
---
# Pi Spool Retirement Test Specs

These tests pin one-way retirement of Quill-owned legacy Pi spool artifacts without importing them.

## Cutover sequencing

Retirement waits for persisted-source reconciliation and an exact reporter generation accepted after reload.

## Owned artifact cleanup

Retirement claims and removes only dead or already-claimed Quill spool files, preserves live and foreign files, records the no-import gap, and completes only after every owned writer exits.

## Typed retirement gap

Provider health maps legacy drop, corrupt-record, and retirement-without-import gap codes to the typed spool error state exposed by integration status.

## Symlink boundary

Retirement rejects a symlinked spool root and leaves every file in its external target untouched.
