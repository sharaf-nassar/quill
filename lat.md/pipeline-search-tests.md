---
lat:
  require-code-mention: true
---
# Pipeline Search Tests

Regression coverage for source-owned Session Search, recoverable migration, and bounded search/context responses.

## Source Ownership

Canonical retained sources and host-qualified pushed sessions have independent indexed ownership.

`sessions::pipeline_tests::pipeline_duplicate_sources_and_remote_survive_replace_and_prune` retains two physical Claude sources sharing native session/message ids plus an independently pushed remote document. Replacing and pruning one canonical source cannot erase the other two. Remote ownership is provider/host/session qualified, separate from canonical retained keys. Production Pi keys use `pi:session:v1`, not the root inventory name `pi:sessions`; the existing Pi header-replacement prune test now uses the production key constructor.

`sessions::pipeline_tests::pipeline_codex_duplicate_sources_and_remote_hosts_are_independent` repeats replacement/pruning with duplicate Codex sources and verifies that deleting one pushed host leaves the other host and retained source intact.

Schema 10 stores an exact indexed `source_key`. A retained source replacement deletes only that key. Complete root inventories prune matching provider/path ownership; incomplete roots never grant deletion authority. The existence/header guard still protects a live commit newer than the inventory.

## Committed Ownership Recovery

The Tantivy commit, not a separately flushed JSON sidecar, owns successful source checkpoints.

`sessions::pipeline_tests::pipeline_missing_corrupt_lagging_sidecar_recovers_committed_sources` drops the index after source commit without a sidecar save, then reopens with missing, corrupt, or lagging sidecar bytes. The committed source remains discoverable and can be pruned without deleting pushed history.

One internal checkpoint document per retained source commits in the same Tantivy transaction as its messages, carrying fingerprint, canonical path, native session id and hints. `document_kind` isolates checkpoints from search responses. Startup reads only checkpoint postings; `index_state.json` is no longer ownership authority. Per-source commits serialize only one source, not the entire corpus. The sidecar remains batched and atomically replaced for negative/rejection checkpoints and older-reader compatibility.

## Search Bounds

Date boundaries and pagination are validated centrally before Tantivy collectors allocate.

`sessions::pipeline_tests::pipeline_date_to_includes_whole_day_and_rejects_invalid_dates` includes 23:59:59 UTC on a plain `date_to`, treats RFC3339 as an instant, and rejects malformed dates rather than broadening the range. A plain upper date is exclusive next UTC midnight.

`sessions::pipeline_tests::pipeline_pagination_is_nonzero_and_bounded` rejects zero page size and overflow/oversized offsets before Tantivy allocates collectors. Effective page size caps at 100, offsets use that effective size, and offset plus limit cannot exceed 10,000. HTTP malformed integers and invalid search bounds/dates return 400; IPC returns the central validation error.

## Context Bounds And Identity

Source-qualified context retains its requested message within explicit window and serialized byte limits.

`sessions::pipeline_context_tests::pipeline_context_selects_source_and_keeps_target_with_bounded_unicode_tools` selects the exact canonical source among duplicate native ids, rejects ambiguous old-client requests and absent target ids, and measures serialized JSON with escaped control characters and Unicode. It retains the requested message and bounded nonempty tool metadata, with explicit response/message `truncated` flags.

`sessions::pipeline_context_tests::pipeline_context_extreme_window_is_explicitly_capped` requests `usize::MAX` and receives at most 20 neighbors per side (41 messages), including the target, with truncation visible. Responses cap at 64 KiB including JSON escaping and session-name enrichment. Initial content/tool previews cap at 16/8 KiB, tool names at 1 KiB, and presentation metadata at 512 bytes. If necessary, farthest neighbors are removed before the target text is shortened. UTF-8 boundaries and truncation markers survive.

Search hits carry additive `source_key`; HTTP accepts `source_key`, IPC accepts optional `sourceKey`. New clients never resolve a remote or unattributed hit through a different local source. Legacy clients omitting the selector retain registry lookup when ownership is unambiguous. Context identity longer than 1024 bytes rejects instead of silently changing the requested id.

## Preservation First Migration

Schema upgrades preserve unattributed history and recover interrupted directory promotion.

`sessions::pipeline_migration_tests::pipeline_migration_preserves_legacy_remote_and_exact_project_filter` migrates a real v9-shaped index, retaining all stored content, provider facets, and the nonstored exact cwd field recovered from postings. Reopen is idempotent. v9 has no local-versus-pushed ownership discriminator: those documents become `legacy:unattributed`, never candidates for source pruning. They may coexist with newly indexed retained messages and their context may be unavailable; preserving irreplaceable remote history outranks guessing ownership.

`sessions::pipeline_migration_tests::pipeline_migration_recovers_before_between_and_after_renames` exercises failed staging, interruption between renames, and interruption after the new index replaces the old one. Migration commits and validates a sibling staged index before closing all mmap/writer handles and renaming directories. Startup validates a promoted index or restores the backup. No blind schema-version rebuild is permitted. The existing marker test now asserts unknown files survive.

`sessions::pipeline_migration_tests::pipeline_migration_preserves_sparse_project_postings_across_batches` migrates 771 documents across three distinct cwd posting lists and reopens the result. Each project must retain exactly its 257 documents across batch boundaries. Tantivy postings start at their first matching document; migration seeks only forward when that document precedes the current batch, never backward to a smaller batch start.

Migration batches at most 256 documents and stops after 8 MiB of stored text (plus at most one document); cwd terms are scanned per batch. This is bounded content buffering, not an RSS guarantee or a production-corpus migration measurement. The ignored `sessions::tests::measure_session_index_schema_migration_on_pinned_corpus` now seeds an actual v9 schema: 14,030 synthetic documents (80 sessions, 655 injected-context documents), 1,844,014 index bytes before migration, and 3,530 ms migration wall time in the lane run. Its pinned manifest SHA-256 is `0489da2b94fe813d785f8b5bc4ed2f871b3f0732cde6aab5334c55788f9f673e`. Tantivy's writer budget is unchanged. Index commit semantics follow Tantivy 0.25 atomic meta.json updates; migration-directory parent fsync/power-loss durability is not claimed.

## Context Failure UI

Failed context requests leave loading and remain isolated to the selected physical source.

`scripts/pipeline-search-context.test.mjs` drives out-of-order requests, source switching, failure rendering, and retry clearance through the actual selection handler.

Context errors are keyed by provider/source/host/session/message, cleared on selection/retry, and rendered as an accessible context-unavailable alert rather than permanent loading. A late failure for a previously selected physical source cannot appear on the current source. Truncation notices use existing detail styles and accessible status text. The regression runs in the serial frontend test suite; integration validation uses an isolated checkout with existing dependencies, never tooling against the live Vite checkout.
