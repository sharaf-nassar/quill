---
lat:
  require-code-mention: true
---
# Pi Notify Index Test Specs

These tests pin Pi startup, watcher, and extension-notified search indexing, shared analytics admission, and provider isolation.

## Notify Identity And Parent

A Pi notify reads only its named transcript and indexes messages under the pushed session id and lineage parent id.

## Notify Tool And Skill Rows

A Pi notify persists parsed `tool_actions` and `skill_usages`, then admits the same validated source to retained reconciliation.

Startup and watcher reconciliation can authoritatively replace those rows later under the same owner.

Write and edit inputs carry their line counts through to code stats, a `tool_detail` row keeps its identity while its payload columns drop at the bind, and a SKILL.md read attributes to its skill. Re-notifying the same transcript replaces the rows instead of doubling them, and the fast-path rows still land when the search index is absent.

## Owned Row Builder Shared With Retained Parsing

Pi's notify path and retained reconciliation build `tool_actions` and `skill_usages` rows through the same identity-aware builder.

The action-key fallback chain and skill fan-out therefore produce identical shapes under one canonical Pi source key; retained Claude/Codex sources continue supplying their native chain identity.

## Configured Root Containment

Pi notify rejects a transcript outside the configured Pi session root and never admits it through the legacy search-only fallback.

## Watcher Recovery

The filesystem watcher registers the configured Pi root with Claude and Codex, preserves provider identity through debounced changed-source admission, and uses whole-root recovery for remove, rename, overflow, late-root, and periodic rescan signals.

## Startup Search Recovery

Session Search startup inventory scans persisted Pi files without requiring a prior notify, indexes each supported user/assistant message once, and retains Pi provider/session identity.

## Shared Coordinator Admission

Validated Pi sources enter the existing provider-plus-source coordinator with transcript work armed and model work unarmed. Pi does not create a second queue, permit, retry, or backoff implementation.

## Retired Spool Isolation

Migration 46's durable `pi_spool_cleanup_pending` marker prevents production startup from spawning the legacy spool drain.

Persisted sessions remain the only reconciliation source; direct legacy drain tests stay available until deployment cutover owns artifact deletion.

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
