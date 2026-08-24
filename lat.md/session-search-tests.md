---
lat:
  require-code-mention: true
---
# Session Search Test Specs

These tests pin Pi conversation-role filtering, provider-native search roles, injected-context indexing, session-name enrichment, bounded model-facing results, and concurrency-safe Tantivy resource use.

## Index Test Resource Budget

Production `SessionIndex::open_or_create` retains its 50 MB Tantivy writer heap. Index tests use a 15 MB writer heap so independent temporary indexes remain safe under default test parallelism.

The shared test opener selects one Tantivy writer worker instead of production's three.

## Conversation Role Guard

Pi search admits only user, assistant, and `custom_message` documents, while Claude and Codex keep intentional provider-native roles such as Codex collaboration senders.

`sessions::tests::search_excludes_non_conversation_roles` pins the excluded legacy roles and the retained provider-native ones.

## Injected Context Search

Pi `custom_message` entries index as `custom_message` documents in source order, carrying their `customType` as searchable `custom_type` metadata and their bounded content.

`display:false` entries index the same way because `display` governs Pi's TUI only. They emit no session or runtime event, and non-context `custom` entries — including `quill-tracking` — stay out of search. `sessions::tests::pi_custom_messages_index_with_their_custom_type_and_emit_no_events` pins ordering, role, `custom_type` search, event silence, and the negative cases.

## Injected Context Analytics Neutrality

Adding `custom_message` entries to a Pi source changes no session event, response time, or tool row.

`transcript_analytics::tests::pi_custom_messages_leave_runtime_and_turn_evidence_unchanged` compares one source with and without injected context and requires identical evidence.

## Pi Session Name Capture

The last `session_info` name by source ordinal persists to the source's registry row, and a cleared name clears it.

`transcript_analytics::tests::pi_session_name_takes_the_last_info_entry_and_clears_when_emptied` pins last-wins capture, persistence through atomic snapshot replacement, and convergence to NULL on rename to an empty name.

## Session Name Response Enrichment

Search hits and session context carry a nullable `session_name` joined from the analytics registry after the index query, never from a stored document.

Lookups are deduplicated per response and chunked, so a page costs a bounded number of indexed reads and a batch wider than one chunk keeps its tail. `storage::tests::session_name_enrichment_uses_one_bounded_registry_lookup` pins per-provider resolution, absent names staying NULL, and the chunk boundary.

## Legacy Pi Role Cleanup

Opening an existing index deletes only Pi documents with non-conversation roles while preserving Pi conversation messages, Pi `custom_message` documents, and documents from other providers.

The cleanup is one-time: a later open leaves a newly indexed legacy document alone. `sessions::tests::opening_index_removes_only_legacy_pi_non_conversation_documents` pins both.

## Schema Rebuild Measurement

The schema-8 rebuild cost is measured on the pinned audit-window corpus rather than asserted.

`sessions::tests::measure_session_index_schema_rebuild_on_pinned_corpus` is an ignored measurement that reindexes 14,030 documents — including 655 injected-context documents — through one writer and one commit, and reports wall time. Recorded evidence lives in `specs/030-pi-analytics-migration-measurement.md`.

## Compact AI Results

Compact search responses return snippet and identity fields — including the nullable `session_name` — without full content, stopping before the serialized response exceeds 32 KiB.
