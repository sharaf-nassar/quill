---
lat:
  require-code-mention: true
---
# Pi Session Parser Test Specs

These tests pin the narrow Pi parser kept for notified search indexing and bounded header probes.

## V3 Message Entries

V3 headers and message entries retain the identity, cwd, timestamps, ids, parent links, and message values needed by indexing.

## V2 Hook Messages

V2 message entries with the retired `hookMessage` role parse as custom-role messages, matching Pi's v2-to-v3 migration.

## Unsupported V1

V1 sessions return an explicit unsupported-version error because they lack stable tree ids and parent links.

## Malformed And Unknown Input

Malformed lines, non-message entries, and invalid messages do not prevent later valid messages from parsing.

## Ephemeral Sessions

An absent session-file path or a missing file returns no session, covering Pi's non-persisted ephemeral mode without filesystem mutation.

## Bounded Header Probe

The header probe reads at most 64 KiB and accepts only supported v2/v3 session headers for live identity fallback.
