---
lat:
  require-code-mention: true
---
# Pi Model Usage Test Specs

Pi usage tests pin persisted-session reconciliation, canonical ownership, and the schema-46 cut-over.

## Native Usage Migration

Opening a schema-42 database adds nullable event identity and five native cost fields, preserves existing observations, creates the Pi-only dedupe index, and records schema 43 once.

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

One persisted Pi snapshot replaces every source-owned runtime, tool, receipt, token, usage, rollup, and registry row in one SQLite transaction.

Lifecycle evidence participates only when present and ordered after the committed lifecycle already stored for that session.

A final registry failure rolls every table back, identity drift retains last-good, and an empty replacement clears only its source-owned analytics evidence while preserving both registries, a sibling source, and any newer committed lifecycle when lifecycle evidence is absent. A superseded process cannot close the newer process; a persisted open process rehydrates as `recovering` until its own end appears.
