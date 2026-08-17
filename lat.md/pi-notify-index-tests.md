---
lat:
  require-code-mention: true
---
# Pi Notify Index Test Specs

These tests pin extension-notified Pi search indexing, the analytics rows that same parse persists, and provider isolation.

## Notify Identity And Parent

A Pi notify reads only its named transcript and indexes messages under the pushed session id and lineage parent id.

## Notify Tool And Skill Rows

A Pi notify persists the parsed transcript's `tool_actions` and `skill_usages`, which no other path produces for Pi.

Write and edit inputs carry their line counts through to code stats, a `tool_detail` row keeps its identity while its payload columns drop at the bind, and a SKILL.md read attributes to its skill. Re-notifying the same transcript replaces the rows instead of doubling them, and the rows still land when the search index is absent.

## Configured Root Containment

Pi notify rejects a transcript outside the configured Pi session root and never admits it through the legacy search-only fallback.

## Watcher Exclusion

The filesystem watcher registers only Claude and Codex roots, so Pi indexing depends on extension notify delivery.

## Watcher Search Recovery

Claude and Codex watcher recovery scans refresh Session Search while Pi remains excluded from root scanning.

## No Root Scan

Provider-qualified session lookup returns no Pi transcript instead of walking every file to compare header ids.

## Message Extraction

The narrow parser extracts each user and assistant message once by entry id with header cwd and project metadata.

## Tool Result Correlation

Pi assistant tool calls populate tool, file, command, and code-change metadata. Matching results attach at 10 KiB, command previews stay at 300 bytes, and result entries never become search documents.

## Provider Safe Search

Pi search hits retain provider, project, and host metadata, while provider facets keep Pi identity instead of falling back to another provider.

## Working Directory Filter

An absolute project filter matches exact indexed cwd identity, so projects with the same final directory name do not leak into each other's results.

## Search Schema Rebuild

Opening an index from schema version 6 removes its old contents and records version 7 so stored metadata and cwd filtering are available after reindexing.

## Provider Safe Cleanup

Reindex cleanup deletes Pi documents only, even when another provider uses the same session id.

## Demo Root Isolation

Demo mode without a Pi override resolves an empty placeholder instead of the persisted production root.
