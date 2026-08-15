---
lat:
  require-code-mention: true
---
# Pi Notify Index Test Specs

These tests pin extension-notified Pi search indexing and provider isolation.

## Notify Identity And Parent

A Pi notify reads only its named transcript and indexes messages under the pushed session id and lineage parent id.

## Configured Root Containment

Pi notify rejects a transcript outside the configured Pi session root and never admits it through the legacy search-only fallback.

## Watcher Exclusion

The filesystem watcher registers only Claude and Codex roots, so Pi indexing depends on extension notify delivery.

## Watcher Search Recovery

Claude and Codex watcher recovery scans refresh Session Search while Pi remains excluded from root scanning.

## No Root Scan

Provider-qualified session lookup returns no Pi transcript instead of walking every file to compare header ids.

## Message Extraction

The narrow parser extracts each text-bearing Pi message once by entry id with header cwd and project metadata.

## Provider Safe Search

Pi search hits and provider facets retain Pi identity instead of falling back to another provider.

## Provider Safe Cleanup

Reindex cleanup deletes Pi documents only, even when another provider uses the same session id.

## Demo Root Isolation

Demo mode without a Pi override resolves an empty placeholder instead of the persisted production root.
