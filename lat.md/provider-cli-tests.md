---
lat:
  require-code-mention: true
---
# Provider CLI Tests

Provider process checks preserve host-tool behavior when Quill runs from a packaged desktop environment.

## AppImage Child Environment

Linux AppImage children omit the inherited library override, while ordinary launches preserve it and Quill's own environment remains unchanged.

The regression re-executes only its test in isolated processes with and without `APPIMAGE`. A real shell child reports `LD_LIBRARY_PATH` after the shared command is converted to Tokio. This pins both synchronous and asynchronous command configuration without unsafe global environment mutation in the test runner.

## Sandbox Environment Replay

Claude's sandbox wrappers retain the inner command's explicit library-path removal, replacing any conflicting wrapper override.

The regression inspects Tokio's underlying command after environment replay. It needs no installed sandbox binary and pins the same replay used by both bwrap and sandbox-exec.
