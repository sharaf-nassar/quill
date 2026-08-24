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
