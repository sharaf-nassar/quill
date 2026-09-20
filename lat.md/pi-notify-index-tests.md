---
lat:
  require-code-mention: true
---
# Pi Notify Index Test Specs

These tests pin Pi startup, watcher, and extension-notified search indexing, shared analytics admission, and provider isolation.

## Notify Identity And Parent

Canonical Pi notify identity must match the persisted header; valid pushed parent metadata remains searchable.

A canonical Pi notify reads only its named transcript; its pushed session id must equal the persisted header id. Mismatch returns HTTP 400 before enqueue or either consumer writes, preserving all last-good data. `server::observed_subagent_tests::pi_notify_requires_native_identity_and_preserves_valid_pushed_search_parent` covers rejection and matching-native-ID success with pushed Search parent metadata. This intentionally rejects the former pushed-ID alias behavior; the deployed extension already prefers header identity.

The same test proves oversize host or lineage ID/reason (>256 bytes), or project/cwd/git branch (>4096 bytes), returns HTTP 400 without admission or consumer writes. Project and branch strings above 256 but within 4096 bytes remain accepted; project can be Pi's full cwd. Empty optional hints retain absent/clear semantics. Malformed JSON headers, headerless or empty files, and absent source files are tested separately from native-ID mismatch: none may be admitted or mutate prior Search/analytics state.

## Notify Tool And Skill Rows

Canonical Pi notify delivers complete source-owned tool and skill evidence independently of Search availability.

A canonical Pi notify admits one retained source to complete reconciliation, which atomically replaces `tool_actions` and `skill_usages` beside the other source-owned evidence. It does not launch a separate two-table fast path or parser. Startup and watcher use the same boundary and canonical owner.

Write and edit inputs carry their line counts through to code stats, a `tool_detail` row keeps its identity while its payload columns drop at the bind, and a SKILL.md read attributes to its skill. Re-notifying the same transcript replaces the rows instead of doubling them, and complete tool/skill evidence still lands when the Search index is absent. `storage::tests::tool_detail_rows_store_no_payload_while_siblings_keep_theirs` pins that detail rows retain `is_error` and `result_image_count` while all three payload columns are NULL.

## Owned Row Builder Shared With Retained Parsing

Pi's notify path and retained reconciliation build `tool_actions` and `skill_usages` rows through the same identity-aware builder.

The action-key fallback chain, result evidence, and skill fan-out therefore produce identical shapes under one canonical Pi source key; retained Claude/Codex sources continue supplying their native chain identity. `transcript_analytics::tests::owned_tool_rows_differ_only_by_owner_identity` pins the shared action keys plus `is_error`, `details_json`, and `result_image_count`.

## Configured Root Containment

Pi notify rejects a transcript outside the configured Pi session root and never admits it through the legacy search-only fallback.

## Watcher Recovery

The filesystem watcher registers the configured Pi root with Claude and Codex, preserves provider identity through debounced changed-source admission, and uses whole-root recovery for remove, rename, overflow, late-root, and periodic rescan signals.

Each recovery pass also admits sources whose mtime advanced since the previous pass, plus bounded durable model obligations independent of that watermark. An ahead-of-time watermark cannot hide never-seen or unchanged transiently failed sources. See [[pipeline-recovery-tests]].

## Startup Search Recovery

Session Search startup inventory scans persisted Pi files without requiring a prior notify, indexes each supported user/assistant message once, and retains Pi provider/session identity.

## Shared Coordinator Admission

Pi notify and watcher admission share canonical source work, with no parallel parsing queue.

Validated Pi sources enter the existing provider-plus-source coordinator with transcript work armed and model work unarmed. Search shares the transcript job and committed-source freshness checks; Pi creates no parallel source registry or parsed cache. The fixed decoder and source lifetime budget are shared across providers.

## No Root Scan

Provider-qualified session lookup returns no Pi transcript instead of walking every file to compare header ids.

## Message Extraction

The narrow parser extracts each user and assistant message once by entry id with header cwd and project metadata.

## Tool Result Correlation

Pi assistant tool calls populate tool, file, command, and code-change metadata. Matching results attach at 10 KiB, command previews stay at 300 bytes, and result entries never become search documents.

`sessions::tests::pi_tool_result_evidence_is_bounded_and_last_write_wins` pins last-result-wins `is_error`, object-only details JSON at or below 10 KiB, oversize or malformed details as NULL, image-block counts without bytes, and input-derived code line counts unchanged by `details`.

## Provider Safe Search

Pi search hits retain provider, project, and host metadata, while provider facets keep Pi identity instead of falling back to another provider.

## Working Directory Filter

An absolute project filter matches exact indexed cwd identity, so projects with the same final directory name do not leak into each other's results.

## Search Schema Migration

Opening an older index preserves existing documents through a staged schema-10 migration.

Unknown ownership remains explicitly unattributed; schema-marker mismatch cannot delete history. Recoverable directory promotion and exact cwd preservation are covered by [[pipeline-search-tests]].

## Provider Safe Cleanup

Reindex cleanup deletes only the canonical Pi source owner, even when another physical file or pushed host uses the same provider and session id.

## Demo Root Isolation

Demo mode without a Pi override resolves an empty placeholder instead of the persisted production root.
