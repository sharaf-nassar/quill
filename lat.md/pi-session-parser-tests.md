---
lat:
  require-code-mention: true
---
# Pi Session Parser Test Specs

These tests pin Pi 0.84.1 session-file decoding before watcher and analytics integration use it.

## V3 Tree Entries

V3 headers and supported entries retain metadata, timestamps, ids, and parent links, while the active path follows the last file-order entry to its root.

## V2 Hook Messages

V2 message entries with the retired `hookMessage` role parse as custom-role messages, matching Pi's v2-to-v3 migration.

## Unsupported V1

V1 sessions return an explicit unsupported-version error because they lack stable tree ids and parent links.

## Summaries

Compaction and branch-summary entries retain the fields needed to reconstruct summarized context and branch provenance.

## Malformed And Unknown Input

Malformed lines, unsupported future types, and invalid known entries do not prevent later valid entries from parsing.

## Ephemeral Sessions

An absent session-file path or a missing file returns no session, covering Pi's non-persisted ephemeral mode without filesystem mutation.

## Parent Path Resolution

An absolute Pi `parentSession` transcript path resolves to the parent's stable header id inside the same session root.

## Invalid Parent Chains

Missing, external, malformed, relative, or cyclic Pi parent chains resolve as unlinked rather than creating partial lineage.
