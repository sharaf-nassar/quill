---
lat:
  require-code-mention: true
---
# Pi Session Parser Test Specs

These tests pin the shared persisted Pi parser used by search indexing and source snapshots, plus bounded notify header probes.

Production Pi reads stream physical JSONL records instead of buffering the whole file. Original bytes, including ignored payloads and malformed lines, feed SHA-256; open-handle and path stability are checked before acceptance. Limits are 4 GiB input, 256 MiB per record, 100000 records, and 256 MiB serialized retained evidence. Exceeding a limit rejects the new version without truncating conversation text or replacing last-good rows. These limits bound input and retained representation, not an exact allocator-byte ceiling.

Tool output retains its existing 10 KiB preview, oversized tool details remain absent under the existing rule, and image payloads and thinking bodies are discarded after their type evidence is recorded. Native usage, tools, summaries, ordinals, and complete user/assistant text remain available.

## V3 Message Entries

V3 parsing retains native message, model-change, and summary evidence with original source order.

Headers and entries preserve identity, cwd, timestamps, ids, parent links, model values, and usage needed by indexing and snapshots. Compaction and branch-summary entries keep their exact JSON value for retained analytics without becoming conversation messages. `thinking_level_change` entries retain their `thinkingLevel` and source ordinal, `session_info` entries retain every observed name with its ordinal so the last one can win, and `custom_message` entries retain their `customType`, content, and ordinal; `pi_session::tests::parses_v3_header_and_message_entries` pins those shapes.

## Retained Thinking Event Classification

Retained Pi assistant blocks emit `asst_thinking` before text and tool-use siblings, including empty blocks. A thinking-only entry stays out of search but remains a runtime event.

`sessions::tests::pi_retained_thinking_events_are_ordered_and_keep_thinking_only_messages` pins event ordering and the thinking-only exception.

## V2 Hook Messages

V2 message entries with the retired `hookMessage` role parse as custom-role messages, matching Pi's v2-to-v3 migration.

## Unsupported V1

V1 sessions return an explicit unsupported-version error because they lack stable tree ids and parent links.

## Malformed And Unknown Input

Malformed lines, unknown custom entries, and invalid native messages do not prevent later valid evidence from parsing.

Exact `quill-tracking` entries are different: malformed or unsupported tracking schemas fail the source rather than silently dropping durable lifecycle evidence.

## Persisted Tracking Entries

Supported `quill-tracking` entries decode through the exact protocol-v2 validator while preserving entry identity and source ordinal.

Native message usage, model-change, tool, skill, lifecycle, receipt, and search evidence remain available from the same parse; tracking rows never become searchable content, and invalid lifecycle tracking produces a typed parse failure. `tool_span`/`thinking_span` entries are routed to the span decoder instead and a malformed span is retained undecoded for the fold's bounded diagnostic.

## Historical Self Resume

Only persisted `session_start` records with reason `resume` and equal previous/current ids have the redundant previous id removed before strict validation.

`pi_session::stream_tests::historical_self_resume_is_repaired_only_on_disk` checks recovery, unchanged live rejection, and rejection of fork self-links and unexpected fields. Source files are never rewritten.

## Streaming Evidence And Hash

Streaming and in-memory parsing produce equal retained evidence and ordinals, while the fingerprint hashes exact original bytes rather than normalized records.

`pi_session::stream_tests::streaming_preserves_evidence_ordinals_and_original_hash` includes malformed and ignored lines and rejects invalid UTF-8.

## Streaming Version Drift

A file changed during streaming is retried before its decoded result can escape; oversized records fail within a bounded read.

`pi_session::stream_tests::streaming_retries_changed_source_and_bounds_records` checks version replacement, original-content hashing, and a sparse oversized record.

## Ephemeral Sessions

An absent session-file path or a missing file returns no session without filesystem mutation; persisted-source snapshots have no evidence to invent for that case.

## Bounded Header Probe

The header probe reads at most 64 KiB and admits only supported v2/v3 session files during notify validation.
