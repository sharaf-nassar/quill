---
lat:
  require-code-mention: true
---
# Pipeline context Rust regression tests

These tests pin bounded asynchronous file and process I/O plus atomic context-store deletion.

## Bounded file indexing

File indexing validates the scoped path, reads at most the requested byte cap plus one byte off the async runtime, preserves valid UTF-8 boundaries through lossy decoding, and reports truncation without loading the whole file.

## One execution deadline

One deadline covers process exit and stdout/stderr drain, so inherited descendant pipes cannot extend the request. Timeout returns partial output and kills the Unix process group.

Cancellation uses the same Unix process-group guard. Windows has no process-group primitive in this implementation, so `kill_on_drop` covers the direct child while inherited descendant pipes stop being drained at the deadline.

Large or JSON-expensive output returns bounded previews and an indexed `source:N` reference without changing the small-output field names.

## Indexed-output persistence failure

After command side effects, an indexing failure returns truthful execution status, a bounded preview, and an explicit non-retry persistence error.

Command completion never becomes an ambiguous HTTP failure that could invite an unsafe retry.

## Fetch-cache persistence failure

A failed cache write returns the successfully indexed source and an explicit `cacheError`, not an ambiguous failure after indexing commits.

`context_store::tests::fetch_cache_failure_preserves_indexed_source_response` injects a real SQLite cache-insert failure and verifies HTTP 200, the usable source reference, retained chunk content, and absent cache row without network access.

## Transactional purge

Source and full-store purges delete fetch-cache references before sources and commit all related deletes atomically. Any delete failure rolls the transaction back so the prior source, chunks, and cache references remain usable.
