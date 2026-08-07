---
lat:
  require-code-mention: true
---
# AppImage Integration Tests

These tests protect Quill's user-space AppImage install and refresh invariants without touching a live desktop installation.

## Automatic Refresh

Automatic refresh tests use temporary AppImage fixtures to verify version-aware replacement decisions and file effects.

### Keeps Newest Installed Version

A newer loose AppImage atomically replaces an older integrated copy, while equal and older loose versions preserve it and a launch from the target refreshes version metadata.
