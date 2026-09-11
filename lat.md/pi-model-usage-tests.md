---
lat:
  require-code-mention: true
---
# Pi Model Usage Test Specs

Pi usage tests pin persisted-session reconciliation, canonical ownership, and the forward-only analytics schema cut-overs through migration 48.

## Analytics Capture Migration

Migration 48 installs the provider-neutral analytics evidence foundation without populating later Pi producers.

Opening a schema-47 database first publishes a verified schema-47 backup, then rebuilds `model_usage_observations` with the `turn`/`token`/`summary` CHECK while preserving row ids, values, the source-record uniqueness constraint, and all seven named indexes. It adds nullable reasoning/outcome/savings/duration evidence, nullable tool error/detail/image/duration evidence, `session_setting_events`, nullable transcript-source `session_name`, and a `turn`/`summary` token-snapshot kind whose legacy default is `turn`; then it rearms only `pi_transcript_analytics_reingest_pending` and records version 48 once, leaving unchanged Claude and Codex roots on their freshness paths.

The migration test inserts old-shape model, tool, source, and token-snapshot rows before opening the real migration path. It proves old rows remain byte-accountable, summary observations and snapshots plus unknown stop-reason text are accepted, other kinds remain rejected, additive columns start NULL, legacy snapshots stay turns, setting identity is unique by `(provider, source_key, setting, source_ordinal)`, and reopen does not re-enter migration 48.

## Analytics Migration Backup Preflight And Recovery

Migration backup, disk preflight, retry, and manual restore share one version-parameterized recovery contract.

The schema-45/pre-46 and schema-47/pre-48 rebuilds use the same `VACUUM INTO` backup path and verification routine. Before backup or rebuild, free space must be at least twice the current database file size; an unreadable or insufficient probe fails before DDL. Schema 47 publishes to `/absolute/path/to/usage.db.schema-47.backup`, including committed WAL state, and verifies `PRAGMA quick_check`, exact schema version, file fsync, atomic rename, directory fsync on Unix, and a second post-publish verification.

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

## Pi Thinking Events And Setting Timeline

Retained Pi thinking blocks emit ordered `asst_thinking` events, while `thinking_level_change` records become source-owned `thinking_level` rows.

The active value is the latest `(timestamp, source_ordinal)` at or before an assistant record; state before its first observation is NULL, and repeated levels remain separate rows. `sessions::tests::pi_retained_thinking_events_are_ordered_and_keep_thinking_only_messages` pins event ordering, and `transcript_analytics::tests::pi_thinking_level_changes_replace_atomically_and_order_by_timestamp_ordinal` pins retained parsing, atomic replacement, and the lookup rule.

## Pi Span Receipt Folding

Persisted span receipts fold into `tool_actions.duration_ms` and summed per-message `reasoning_duration_ms`; malformed spans degrade to NULL plus one bounded diagnostic.

Two thinking blocks on one message sum, a repeated receipt for the same block or tool call id replaces the earlier one, an inverted or field-missing span leaves its duration NULL and counts into `malformed_spans` with the first offending source ordinal, and spans never become event receipts. A file from an older reporter carries no spans, so every duration is NULL and the diagnostics stay empty. `transcript_analytics::tests::pi_span_receipts_fold_into_durations_and_malformed_spans_stay_null` pins each rule.

## Remaining Analytics Evidence Foundation

Later Pi analytics producers remain explicitly empty on the pinned parity corpus.

`reasoning_duration_ms` and tool `is_error`/`details_json`/`result_image_count`/`duration_ms` remain `None` on this span-less corpus while existing runtime, tool, skill, thinking-event, and setting extraction stays unchanged. The corpus carries no `session_info` entry, so its `session_name` stays NULL as unobserved evidence rather than an empty name. `transcript_analytics::tests::remaining_pi_analytics_evidence_foundation_stays_empty` pins the positive parity-corpus thinking and setting evidence plus the remaining NULL fields.

## Summary Usage Read Surface

The models overview exposes unattributed summary spend as its own reconciling bucket, never a model row.

`storage::tests::model_overview_summary_usage_reports_unattributed_summary_spend` pins `summary_usage` counting one model-less summary observation with its tokens, that spend staying inside `total_tokens` while never joining attribution, turn counts, or the per-model rows, attributed rows plus the bucket reconciling the corpus total, and the provider filter scoping the bucket.

## Turn Outcome Aggregation

`get_turn_outcomes` aggregates persisted stop-reason and error-flag evidence per session, per attributed model, and per fixed window with NOT NULL denominators.

`storage::tests::turn_outcome_aggregation_uses_not_null_denominators` pins aborted/error/length counts against a stop-reason denominator that excludes NULL rows, the separate error-flag denominator, a factual null-model bucket for pre-attribution evidence, every grouping summing back to totals, evidence-free windows being omitted, summary rows staying out entirely, and the provider filter returning empty groupings.

## Session Breakdown Analytics Evidence

Sessions rows carry the registry session name and the range-scoped failed tool-call count as nullable joined evidence.

`storage::tests::session_breakdown_joins_names_and_tool_failure_counts` pins `populate_session_analytics_evidence` resolving a named Pi registry row, measured failures counting only in-range `is_error = 1` tool rows, all-success sessions reading a real zero, and sessions without error evidence staying NULL rather than zero.

## Pi Scoped Reingest Marker

Historical Pi analytics backfill must bypass freshness only for Pi roots and clear its marker only after complete Pi success.

`transcript_analytics::tests::pi_scoped_reingest_retries_and_clears_once_after_success` seeds unchanged Claude and Pi sources, removes retained Pi reasoning evidence, and arms only `pi_transcript_analytics_reingest_pending`. It proves Claude keeps its fast-path sentinel while Pi reparses, a failing Pi sibling keeps the marker armed across repeated idempotent retries, repairing that sibling restores the expected evidence, the successful pass deletes the marker exactly once, and the next ordinary pass replaces nothing. Migration 48 separately pins that it arms this Pi marker without setting the global all-provider marker.

## Pi Backfill Starvation Budget

The pinned historical Pi backfill must complete without pushing live-fold p95 beyond the existing 10% overhead budget.

`transcript_watcher::tests::measure_pi_backfill_live_fold_p95_on_pinned_corpus` is an explicit ignored measurement over manifest SHA-256 `0489da2b94fe813d785f8b5bc4ed2f871b3f0732cde6aab5334c55788f9f673e` and generated corpus SHA-256 `b889dab75da4ee743fcc37e505d0814a4c137c2c015bd76b907b715e560e032e`: 80 sessions, 30,700 entries, 12,685 assistant messages, and 16,670 tool results. It schedules retained work only after a live fold, keeps the backfill active across every candidate sample, asserts the exact 14-family evidence count vector after backfill and idempotent replay, reports both wall times plus baseline/candidate p95, and rejects overhead above 10%. Results are valid only with no live Quill listener and no concurrent host workload.

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

Lifecycle evidence participates only when present and ordered strictly after the committed lifecycle already stored for that session. The persisted start at the sequence the live wire already committed is that wire's own event read back from disk, so folding it leaves a proven-open row `open` instead of demoting it to `recovering`.

A final registry failure rolls every table back, identity drift retains last-good, and an empty replacement clears only its source-owned analytics evidence while preserving both registries, a sibling source, and any newer committed lifecycle when lifecycle evidence is absent. A superseded process cannot close the newer process; a persisted open process rehydrates as `recovering` until its own end appears.

## Forked Session Tracking Tolerance

Pi's fork and clone copy the parent's entries, `quill-tracking` included, under the child's header, so a child file legitimately carries another session's lifecycle.

[[src-tauri/src/transcript_analytics.rs#build_pi_persisted_evidence]] skips entries whose `session_id` is not the header's, counting them as conflicting-identity diagnostics, and derives receipts and lifecycle from the child's own entries only. A host or provider mismatch remains a hard identity failure.

A file whose tracking cannot decode still fails, but as a content-deterministic failure: the registry records its fingerprint, so the next pass classifies the unchanged file as an unchanged failure instead of re-reading and re-logging it every sweep.
