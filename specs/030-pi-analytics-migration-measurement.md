# Pi analytics migration measurement

Measured 2026-08-24 with the ignored Rust measurement test
`storage::tests::measure_analytics_capture_migration_on_pinned_corpus`.
The timed interval starts immediately before `Storage::init_at` opens a schema-47
copy and ends after migration 48, its verified schema-47 backup, table rebuild,
index recreation, additive DDL, and startup-index repair complete.

## Corpus

The deterministic audit-window manifest is:

```text
pi-analytics-migration-v1
sessions=80
entries=30700
assistant_messages=12685
tool_results=16670
```

SHA-256:
`0489da2b94fe813d785f8b5bc4ed2f871b3f0732cde6aab5334c55788f9f673e`.
The fixture seeds 12,685 `model_usage_observations`, 16,670 `tool_actions`,
and 80 transcript source rows. Its checkpointed schema-47 database is
18,751,488 bytes. The test asserts the manifest hash and post-migration row
counts before reporting a result.

## Environment

- Linux 7.0.0-29-generic x86_64
- AMD Ryzen Threadripper 3970X 32-Core Processor, 64 logical CPUs
- rustc 1.95.0 (59807616e 2026-04-14)
- debug test profile; local filesystem; no concurrent workload

## Result

Command:

```bash
for run in 1 2 3; do
  cargo test measure_analytics_capture_migration_on_pinned_corpus -- --ignored --nocapture
 done
```

| Run | Wall time |
| --- | ---: |
| 1 | 714 ms |
| 2 | 624 ms |
| 3 | 591 ms |

Median: **624 ms**. Observed range: **591-714 ms**. This is startup-path
evidence for the migration and backup on the pinned audit-window corpus, not a
claim about slower disks or databases larger than this fixture.

## Search index schema rebuild

Measured 2026-08-24 with the ignored Rust measurement test
`sessions::tests::measure_session_index_schema_rebuild_on_pinned_corpus`.
Tantivy schema 8 adds the `custom_type` field, so the first open after upgrade
removes the index directory and reindexes every document. The timed interval
starts before `SessionIndex::open_or_create` reopens a schema-7 directory and
ends after the wipe, recreate, legacy-cleanup pass, full reindex, and reader
reload complete.

The same audit-window manifest and SHA-256 pin the corpus. Its document count
is every entry that becomes a search document — `entries - tool_results` =
**14,030** across 80 sessions — of which the **655** observed `custom_message`
entries carry the new `custom_type` field. The seeded schema-7 index directory
is 1,844,108 bytes. The test asserts the manifest hash and the post-rebuild
`custom_type:subagent-notify` hit count before reporting a result.

Environment matches the migration measurement above. Reindexing uses the shared
15 MB single-worker test opener, while production opens 50 MB with three writer
workers, so this is a conservative upper bound.

Command:

```bash
for run in 1 2 3; do
  cargo test measure_session_index_schema_rebuild_on_pinned_corpus -- --ignored --nocapture
 done
```

| Run | Wall time |
| --- | ---: |
| 1 | 3419 ms |
| 2 | 3317 ms |
| 3 | 3378 ms |

Median: **3378 ms**. Observed range: **3317-3419 ms**. Combined with the
migration median above, one first launch on this corpus spends about **4.0 s**
on schema work before search is fully populated. Transcript parsing during that
sweep is ordinary startup-scan cost and is not attributed to the schema bump.
