# Retention Corpus Study Protocol

This maintainer-only protocol studies an explicitly approved local corpus without opening, migrating, copying, or mutating the source database.

## Preconditions

Use a named local `usage.db`, private approval JSON, private workspace, and
explicit cancellation-marker path. Quill must be stopped. Approval names the
custodian, operator, reviewers, expiry, revocation, allowed uses, cleanup,
independence, fixed UTC anchor, retention window, prior state, and concurrency.
No discovery, default path, upload, or transfer is permitted.

```text
cargo run --release --bin retention_corpus -- \
  profile --approval <private-approval.json> --source <usage.db> \
  --workspace <private-dir> --cancel-marker <private-path>
```

The profiler uses read-only SQLite source connections. It rejects non-SQLite,
unrecognized Quill, too-new, aliased, colliding, symlink-escaping, writable, or
changed source inputs before backup or migration. It captures SHA-256, byte
size, filesystem identity, and presence for database, WAL, and SHM before and
after every source read. Any delta is `source_changed`.

## Scratch replay

The only duplication is SQLite online backup stepped in bounded page batches.
`VACUUM INTO` and filesystem copying are forbidden. Migrations, retention
archive, delete, aggregates, and compaction run only on new identity-checked
scratch copies. Archive-off and archive-on have separate copies.

```text
cargo run --release --bin retention_corpus -- \
  replay --manifest <private-manifest.json> --workspace <private-dir> \
  --cancel-marker <private-path>
```

The utility records mechanics, not a classification. Approved measurement later
requires three controlled warm runs per mode plus separately labelled cold
observations. Cutoffs use strict `timestamp < cutoff`; UTC month buckets apply;
non-conforming timestamps remain counted and never retention-eligible.

## Evidence and cleanup

`private-manifest.schema.json` defines exact local evidence. It is private and
must not be committed. Owner-only workspace permissions are verified or fail
closed. Scratch databases, sidecars, archives, and markers are removed by
default. The renderer creates a new scrubbed report only after human privacy
signoff and approval for aggregate publication:

```text
cargo run --release --bin retention_corpus -- \
  render-report --manifest <private-manifest.json> --output <review.md> \
  --privacy-signoff
```

It suppresses category/month cells below 10, retains whole-table totals, rounds
milliseconds to integers, rates to two decimals, and percentages to one decimal.
It removes source paths, identifiers, hostnames, projects, sessions, agents,
payloads, archive rows, and raw errors. Corpus approval is not Git, Beads, or
transfer approval.

## Synthetic and dbstat capability

`synthetic-smoke --workspace <private-dir>` exercises the existing fixture and
emits a non-sensitive checklist; timing is never corpus evidence. `dbstat`
accepts only identity-verified scratch and produces private capability data;
real execution and retain/reject disposition remain deferred.
