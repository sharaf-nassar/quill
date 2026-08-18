---
lat:
  require-code-mention: true
---
# Pi Session Parser Test Specs

These tests pin the shared persisted Pi parser used by search indexing and source snapshots, plus bounded notify header probes.

## V3 Message Entries

V3 headers, message entries, model changes, and original source ordinals retain the identity, cwd, timestamps, ids, parent links, model values, and usage evidence needed by indexing and snapshots.

## V2 Hook Messages

V2 message entries with the retired `hookMessage` role parse as custom-role messages, matching Pi's v2-to-v3 migration.

## Unsupported V1

V1 sessions return an explicit unsupported-version error because they lack stable tree ids and parent links.

## Malformed And Unknown Input

Malformed lines, unknown custom entries, and invalid native messages do not prevent later valid evidence from parsing.

Exact `quill-tracking` entries are different: malformed or unsupported tracking schemas fail the source rather than silently dropping durable lifecycle evidence.

## Persisted Tracking Entries

Supported `quill-tracking` entries decode through the exact protocol-v2 validator while preserving entry identity and source ordinal.

Native message usage, model-change, tool, skill, lifecycle, receipt, and search evidence remain available from the same parse; tracking rows never become searchable content, and invalid tracking produces a typed parse failure.

## Ephemeral Sessions

An absent session-file path or a missing file returns no session without filesystem mutation; persisted-source snapshots have no evidence to invent for that case.

## Bounded Header Probe

The header probe reads at most 64 KiB and admits only supported v2/v3 session files during notify validation.
