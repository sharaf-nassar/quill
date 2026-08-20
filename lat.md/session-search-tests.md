---
lat:
  require-code-mention: true
---
# Session Search Test Specs

These tests pin Pi conversation-role filtering, provider-native search roles, bounded model-facing results, and concurrency-safe Tantivy resource use.

## Index Test Resource Budget

Production `SessionIndex::open_or_create` retains its 50 MB Tantivy writer heap. Index tests use a 15 MB writer heap so independent temporary indexes remain safe under default test parallelism.

The shared test opener selects one Tantivy writer worker instead of production's three.

## Conversation Role Guard

Pi search admits only user and assistant documents, while Claude and Codex keep intentional provider-native roles such as Codex collaboration senders.

## Legacy Pi Role Cleanup

Opening an existing index deletes only Pi documents with non-conversation roles while preserving Pi conversation messages and documents from other providers.

## Compact AI Results

Compact search responses return snippet and identity fields without full content, stopping before the serialized response exceeds 32 KiB.
