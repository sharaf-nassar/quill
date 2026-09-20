# Pipeline Hardening

Ingestion and retrieval preserve source ownership, last-good evidence, and bounded work across files, indexes, SQLite, and subprocesses.

## Source Ownership

A retained file owns its search documents independently of another file or remote producer with the same native session identity.

Provider/session identity describes a conversation, not a deletion key. Canonical source identity owns replacement and pruning. Search must be able to recover ownership from committed index data after a missing, corrupt, or lagging checkpoint sidecar.

Schema migration preserves remote-only documents. Legacy documents whose owner cannot be proved remain explicitly unattributed rather than granting a filesystem sweep permission to delete them. Migration stages and validates the replacement before retiring the old index, and interrupted directory replacement must recover deterministically.

## Inventory and Recovery

A completed filesystem inventory proves absence only within the reconciliation lifetime that protects its generation and pruning decisions.

A live commit after inventory collection must not be deleted because a later sweep allocates a newer generation. Protect the inventory-through-prune decision, not merely the final SQLite transaction. Incomplete discovery never proves absence.

Model jobs complete per source. Recording a diagnostic successfully is not successful ingestion. Permanent, versioned source rejections settle unchanged invalid bytes; transient failures retain recovery obligations independently of source modification time. Healthy siblings do not inherit another source's retry count.

## Working Context Safety

Fetches pin validated public destinations, database mutations roll back atomically, and indexing and execution enforce their limits before allocating or returning large payloads.

Each fetch redirect receives fresh address validation. The connection uses a validated numeric address while HTTP Host and TLS identity remain the original hostname. Ambient proxies cannot replace the validated destination.

Python operations sharing one SQLite connection serialize their complete reads and transactions. Replacement failure rolls back the prior deletion and any new chunks or FTS rows. Source purge removes fetch-cache references before foreign-key actions can null them, and source and full-store deletion are atomic in both implementations.

File indexing reads only the configured byte budget plus overflow evidence, off the async executor. Execution deadlines cover the child and output pipes, including descendants that inherit those pipes. Captured bytes stay bounded. Large indexed output returns a usable source ref and bounded preview rather than repeating the full output and failing after command side effects. A fetch-cache persistence failure also preserves the successfully indexed source reference and reports `cacheError` explicitly.

## Retrieval Bounds

Date ranges, pagination, and context windows have explicit, shared semantics at the retrieval boundary.

A plain end date includes its complete UTC day using an exclusive next-day boundary. Search rejects invalid pagination before constructing collectors or computing unbounded offsets. Context responses bound both message counts and serialized bytes, keep the requested message identifiable, and expose truncation instead of silently implying completeness.

## Research References

The implementation decisions use upstream contracts and the repository's pinned library implementations rather than inferred framework behavior.

- [Tantivy architecture](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md): applications own primary-key and deletion semantics; commits atomically update index metadata.
- [Tantivy 0.25 prepared commits](https://docs.rs/tantivy/0.25.0/tantivy/indexer/struct.PreparedCommit.html): commit payload, commit, abort, and operation stamps.
- [HTTPcore 1.0.9 request extensions](https://github.com/encode/httpcore/blob/1.0.9/docs/extensions.md): numeric-IP connections with original Host and `sni_hostname` preserve TLS hostname verification.
- [HTTPX 0.28.1 transports](https://github.com/encode/httpx/blob/0.28.1/docs/advanced/transports.md): supported transport interfaces and deterministic mock transports.
- [Python sqlite3](https://docs.python.org/3/library/sqlite3.html): connection context managers commit or roll back; shared connections require application serialization.
- [SQLite isolation](https://www.sqlite.org/isolation.html): separate connections hide uncommitted writes, but operations within one connection are not isolated from one another.
- [Tokio process command](https://docs.rs/tokio/latest/tokio/process/struct.Command.html): process groups, direct-child kill-on-drop, and process cleanup responsibilities.
