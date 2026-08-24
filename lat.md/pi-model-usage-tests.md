---
lat:
  require-code-mention: true
---
# Pi Model Usage Test Specs

Pi usage tests pin persisted-session reconciliation, canonical ownership, and the forward-only analytics schema cut-overs through migration 48.

## Analytics Capture Migration

Migration 48 installs the provider-neutral analytics evidence foundation without populating later Pi producers.

Opening a schema-47 database first publishes a verified schema-47 backup, then rebuilds `model_usage_observations` with the `turn`/`token`/`summary` CHECK while preserving row ids, values, the source-record uniqueness constraint, and all seven named indexes. It adds nullable reasoning/outcome/savings/duration evidence, nullable tool error/detail/image/duration evidence, `session_setting_events`, nullable transcript-source `session_name`, and a `turn`/`summary` token-snapshot kind whose legacy default is `turn`; then it rearms `transcript_analytics_reingest_pending` and records version 48 once.

The migration test inserts old-shape model, tool, source, and token-snapshot rows before opening the real migration path. It proves old rows remain byte-accountable, summary observations and snapshots plus unknown stop-reason text are accepted, other kinds remain rejected, additive columns start NULL, legacy snapshots stay turns, setting identity is unique by `(provider, source_key, setting, source_ordinal)`, and reopen does not re-enter migration 48.

## Analytics Migration Backup Preflight And Recovery

Migration backup, disk preflight, retry, and manual restore share one version-parameterized recovery contract.

Every destructive schema migration uses the same `VACUUM INTO` backup path and verification routine. Before backup or rebuild, free space must be at least twice the current database file size; an unreadable or insufficient probe fails before DDL. Schema 47 publishes to `/absolute/path/to/usage.db.schema-47.backup`, including committed WAL state, and verifies `PRAGMA quick_check`, exact schema version, file fsync, atomic rename, directory fsync, and a second post-publish verification.

A failed table rebuild rolls its transaction back to schema 47 with every row intact. Removing the injected blocker resumes migration against the verified backup. The recovery test also replaces the database with that backup and proves startup reapplies migration 48 without data loss.

Restore and verify with Quill stopped:

```bash
rm -f /absolute/path/to/usage.db-wal /absolute/path/to/usage.db-shm
cp /absolute/path/to/usage.db.schema-47.backup /absolute/path/to/usage.db
sqlite3 /absolute/path/to/usage.db 'PRAGMA quick_check; SELECT MAX(version) FROM schema_version;'
```

Expected output before restart is `ok` and `47`; restarting Quill reapplies migration 48 and retains the verified schema-47 backup.

## Analytics Migration Measurement

The migration wall-time measurement uses one hash-pinned audit-window corpus.

`storage::tests::measure_analytics_capture_migration_on_pinned_corpus` is explicitly invoked over SHA-256 `0489da2b94fe813d785f8b5bc4ed2f871b3f0732cde6aab5334c55788f9f673e`: 80 sessions, 30,700 entries, 12,685 model observations, and 16,670 tool rows in an 18,751,488-byte schema-47 database.

Three controlled local runs measured 714 ms, 624 ms, and 591 ms from `Storage::init_at` entry through verified backup, migration, index recreation, and startup-index repair; median 624 ms. Environment, command, scope, and limitations are recorded in `specs/030-pi-analytics-migration-measurement.md`.

## Pi Reasoning And Outcome Evidence

Retained Pi assistant usage captures reasoning and outcome evidence without changing token totals.

`transcript_analytics::tests::pi_reasoning_and_outcome_evidence_preserves_usage_totals` parses nullable `usage.reasoning`, normalizes known stop reasons, stores absent reasoning and stop reason as NULL, records `errorMessage` presence as measured true/false evidence, and retains unknown stop reasons under the 256-byte bound with count-plus-first-ordinal diagnostics. Reasoning stays informational: input/output/cache totals do not add it again.

## Pi Summary Usage Evidence

Compaction and branch-summary usage becomes stable summary observations without invented model identity.

`transcript_analytics::tests::pi_summary_usage_has_stable_identity_and_no_fabricated_model` pins `pi_summary_v1:{session-id-length}:{entry-id}` identity, bounded compaction `tokens_before`, missing model fields with `model_evidence='missing'`, explicit valid provider/model evidence when supplied, and identical rows across reparses.

## Pi Summary Accounting Reconciliation

Summary spend enters every token aggregate while turn counters remain assistant-only.

`transcript_analytics::tests::pi_summary_usage_reconciles_without_turn_inflation` replaces the same pinned source twice and proves five observations reconcile to three turns plus two summaries. Raw observations, model hourly rows, token snapshots, provider token stats, model overview totals, and session history all report 2,032 tokens. Only assistant rows enter token, model, or segment turn counters; the model-less compaction remains 1,300 unattributed tokens.

## Remaining Analytics Evidence Foundation

Later Pi analytics producers remain explicitly empty on the pinned parity corpus.

`reasoning_duration_ms`, tool `is_error`/`details_json`/`result_image_count`/`duration_ms`, setting events, and session name remain `None` or empty while existing runtime, tool, and skill extraction stays unchanged.

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

Expected output is `ok` and `45`; restarting Quill reapplies migrations 46-48 and retains the verified schema-45 backup.

## Persisted Source Atomic Replacement

One persisted Pi snapshot replaces every source-owned evidence family in one SQLite transaction.

The generic snapshot writer binds runtime, tool, setting, receipt, token, usage, rollup, and registry rows, including the new nullable model/tool fields and registry session name. The retention watermark still filters only runtime events, tool actions, and Pi usage, so setting rows remain unpruned until source replacement or source deletion.

Lifecycle evidence participates only when present and ordered after the committed lifecycle already stored for that session.

A final registry failure rolls every table back, identity drift retains last-good, and an empty replacement clears only its source-owned analytics evidence while preserving both registries, a sibling source, and any newer committed lifecycle when lifecycle evidence is absent. A superseded process cannot close the newer process; a persisted open process rehydrates as `recovering` until its own end appears.
