---
lat:
  require-code-mention: true
---
# Pi Session Parser Test Specs

These tests pin the shared persisted Pi parser used by search indexing and source snapshots, plus bounded notify header probes.

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

## Ephemeral Sessions

An absent session-file path or a missing file returns no session without filesystem mutation; persisted-source snapshots have no evidence to invent for that case.

## Bounded Header Probe

The header probe reads at most 64 KiB and admits only supported v2/v3 session files during notify validation.
