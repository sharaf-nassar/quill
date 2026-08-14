---
lat:
  require-code-mention: true
---
# Pi Watcher And Index Test Specs

These tests pin Pi session discovery, search indexing, and provider isolation.

## Message Identity And Extraction

Each Pi message entry is indexed once by entry id under the header session id, with text, parent, cwd, and project metadata retained.

## Recursive Candidate Collection

The persisted Pi session root admits nested JSONL transcripts and ignores other files.

## Provider Safe Search

Pi search hits and provider facets retain Pi identity instead of falling back to another provider.

## Parent Session Search Metadata

Pi search documents store the parent header id resolved from the declared transcript path so child results can navigate to their parent without path leakage.

## Provider Safe Cleanup

Reindex cleanup deletes Pi documents only, even when another provider uses the same session id.

## Root Registration And Refresh

The watcher registers Pi at startup and accepts a changed resolved root for late lifecycle mutations.

## Demo Root Isolation

Demo mode without a Pi override resolves an empty placeholder instead of the persisted production root.
