---
lat:
  require-code-mention: true
---
# Pi Model Usage Test Specs

Pi usage tests pin pushed hints, persisted-session reconciliation, canonical ownership, and the schema-46 cut-over.

## Removal Parity Gate

The scripted Pi corpus proves exact linear totals, explains copied-ancestor fork divergence, and deduplicates replay while unioning disjoint pre-upgrade and pushed message identities.

## Pushed Usage Migration

Opening a schema-42 database adds nullable event identity and five native cost fields, preserves existing observations, creates the Pi-only dedupe index, and records schema 43 once.

## Live Source Migration

Migration 44 moves Pi runtime and pushed model rows to one stable owner without changing evidence identity. Migration 45 compacts state without reopening sealed runtime.

Migration 46 rekeys that owner to the canonical normalized-host-plus-session identity, marks it stale for persisted reconciliation, seeds prior open rows as `recovering`, and preserves event, session, chain, host, timestamp, ordinal, token, UUID, and cost fields. A second open changes nothing.

## Canonical Source Identity

`pi_source_key` normalizes hostname once and hex-encodes both host and session bytes, so hostname case aliases collapse while equal session ids on different hosts and delimiter-shaped identities cannot collide.

## Schema 45 Backup And Ownership Migration

Before migration-46 DDL, an existing database is advanced to schema 45 and copied to the exact sibling path `/absolute/path/to/usage.db.schema-45.backup`.

`VACUUM INTO` includes committed WAL state. Quill verifies `PRAGMA quick_check` and schema version 45, fsyncs, then atomically renames `.building`; stale build files, published backups, and restored databases resume safely. A pinned-reader test proves main-database and WAL-only probes both reach the backup.

Restore and verify with Quill stopped:

```bash
rm -f /absolute/path/to/usage.db-wal /absolute/path/to/usage.db-shm
cp /absolute/path/to/usage.db.schema-45.backup /absolute/path/to/usage.db
sqlite3 /absolute/path/to/usage.db 'PRAGMA quick_check; SELECT MAX(version) FROM schema_version;'
```

Expected output is `ok` and `45`; restarting Quill reapplies migration 46 and retains the verified schema-45 backup.

## Persisted Source Atomic Replacement

One persisted Pi snapshot replaces every source-owned runtime, tool, lifecycle, receipt, token, usage, rollup, and registry row in one SQLite transaction.

A final registry failure rolls every table back, identity drift retains last-good, and an empty replacement clears only its source while preserving both registries and a sibling source. A superseded process cannot close the newer process; a persisted open process rehydrates as `recovering` until its own end appears.

## Replay And Cost Storage

Live delivery and replay of one Pi usage event retain one observation under its canonical host-qualified session source while preserving every token and native per-field plus total cost value.

## Tracking Replay And Live Totals

Replaying one accepted usage envelope leaves one Models contribution and one cumulative LiveTracker token increment.

## Upgrade Coexistence

A resumed session unions saved pre-upgrade rows with post-upgrade pushed rows in raw and completed-rollup Models reads, with the parity gate proving their message identities do not overlap.

## Live Source Retention

Confirmed retention prunes active and closed Pi model, runtime, and response detail behind its cutoff without deleting the live owner. Old replay stays suppressed, while current UUID-keyed evidence inserts once with exact totals.
