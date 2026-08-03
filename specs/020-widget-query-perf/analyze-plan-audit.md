# Bounded ANALYZE plan audit

This audit gates feature 020's planner-statistics change against traced
production SQL on a disposable copy of the frozen corpus.

## Maintenance contract

`compact_database` owns ANALYZE because it already provides the required
blocking ingest-quiesce lease, disk preflight, dedicated writer, and
`spawn_blocking` boundary. A successful run orders work as follows:

1. acquire the blocking ingest-quiesce lease;
2. preflight disk and run VACUUM;
3. set `PRAGMA analysis_limit=1000` on a dedicated writer;
4. run `ANALYZE` in one immediate transaction and require `sqlite_stat1`;
5. reload statistics and flush prepared statements on the long-lived writer;
6. run `PRAGMA wal_checkpoint(TRUNCATE)`; then
7. release the lease and emit the finished result.

Skipped preflight or VACUUM results do not run ANALYZE. Retention compaction
does not inherit this step: a retention run has already made a separately
consented mutation, so an unexpected statistics failure must not obscure its
durable prune result. Result caches remain valid because statistics affect
plans, not query semantics; disposable view readers load new stats when opened.

Unexpected ANALYZE, verification, writer-reload, or checkpoint failures bubble
with the failing operation named. The final checkpoint is attempted even if
writer reload fails, bounding WAL before the lease releases.

## SQLite verification

SQLite documents `analysis_limit=N` as an approximate per-index row-visit
limit for subsequent ANALYZE statements and recommends values from 100 to
1,000 for large databases:

- <https://sqlite.org/pragma.html#pragma_analysis_limit>
- <https://sqlite.org/lang_analyze.html#approximate_analyze_for_large_databases>

A local SQLite 3.45.1 probe applied `1000`, completed ANALYZE, and created
`sqlite_stat1`. The Cargo-pinned production dependency is rusqlite 0.31.0 with
bundled libsqlite3-sys 0.28.0; its checked-in amalgamation identifies SQLite
3.45.0 and contains the same `nAnalysisLimit` implementation. The production
audit applies 1,000 and requires `sqlite_stat1` after bounded analysis.

## Production-SQL protocol

The maintainer-only `audit-analyze` mode receives a fresh writable online
backup. It refuses any copy with nonempty `sqlite_stat1`, pins the feature-020
query clock, and runs the existing 24-hour, 30-day, and 90-day production
endpoint matrix before and after bounded ANALYZE. An optional report path is
opened create-only, serialized through Rust, flushed, and synced before a fail
verdict exits, so a large plan manifest cannot be lost to terminal truncation.

rusqlite tracing is attached only to benchmark reader connections. It captures
expanded SQL after real production parameter binding. Replay groups statements
by original connection, reproduces transaction and temp-table state in order,
and prepares `EXPLAIN QUERY PLAN` against the exact expanded statement. SQL
identity uses a trace-derived shape that replaces quoted and numeric literals;
the manifest retains both exact expanded hashes. This accommodates endpoints
whose live-value binds intentionally sit outside the pinned range clock without
substituting hand-copied SQL.

The complete matrix covers:

- model overview and history;
- token history and runtime;
- code stats and code-stats history;
- host, project, session, skill, and hook breakdowns;
- bucket stats and context savings; and
- Usage, Charts, and Context fan-outs, which additionally exercise provider
  token series, activity series, token stats, and retention policy reads.

Every per-path/shape occurrence count must match. The audit flags increased
`SCAN`, decreased `SEARCH`, new temp B-trees, lost named indexes, output-size
changes, or a cold slowdown above both 5 ms and 25%. Any flagged plan or timing
row makes the command fail instead of shipping ANALYZE. Focused confirmation
alternates exact statless/analyzed production paths eight times per state,
requires canonical output hashes and plan shapes, and applies the same combined
timing gate without weakening either threshold.

## Prior raw-only frozen-corpus result

This historical result records the pre-merge raw-only branch and is superseded
for release eligibility by the merged pending/completed result below.

The run used pinned end `2026-08-02T19:27:43Z`. A fresh SQLite online backup
was 13,525,123,072 bytes, passed `quick_check`, and initially matched the frozen
source byte-for-byte at SHA-256
`c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e`.
Only the disposable copy was made writable.

### Statistics operation

| Measure | Result |
| --- | ---: |
| Bundled SQLite | 3.45.0 |
| `analysis_limit` readback | 1,000 |
| ANALYZE transaction | 2,394.618 ms |
| Complete statistics step | 2,410.762 ms |
| `sqlite_stat1` before | absent |
| `sqlite_stat1` after | 121 rows |
| `sqlite_stat1` SHA-256 | `85ec791dec0c017e487b66e14d11865c83777068322177cb0f1f92df0084fd32` |
| Final checkpoint log/checkpointed frames | 0 / 0 |
| Final `quick_check` | `ok` |

### Complete plan manifest verdict

The traced matrix prepared 232 exact production SQL statements across all 42
endpoint-window and fan-out paths. Per-path/shape occurrence cardinalities
matched before and after. Of these, 219 plans were byte-identical and 13 plan
occurrences changed, representing four unique SQL shapes:

| Shape SHA-256 prefix | Occurrences | Before → after |
| --- | ---: | --- |
| `2c2a9d2b4307` | 4 | Project breakdown retained `idx_token_snap_cwd`, adding timestamp skip-scan bounds; all three temp B-trees stayed fixed. |
| `516c86b6c2ff` | 3 | Represented-provider grouping retained `idx_model_observations_observed_provider`, its source-key lookup, and its GROUP BY temp B-tree while adding a source bloom filter. |
| `aafc60a2e634` | 3 | Model running-now stage added a source bloom filter; all searches, scans, automatic indexes, and temp ordering stayed fixed. |
| `ed67cf161382` | 3 | Model raw-source join added a source bloom filter; both named searches stayed fixed. |

No occurrence increased SCAN count, decreased SEARCH count, introduced a temp
B-tree, or lost a named index. The session-candidate and provider-series
aggregates pin their grouping-oriented token index, while represented-provider
grouping pins its observed-time range index. All three plans are therefore
stable before and after statistics are loaded.

### Endpoint timing verdict

The full timing set contained 39 endpoint-window rows and three fan-outs. No
row crossed both fail-closed thresholds:

| Path | Statless cold | Analyzed cold | Delta | Ratio | Statless warm | Analyzed warm |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Model history, 24h | 14.807 ms | 16.708 ms | +1.901 ms | 1.128× | 0.913 ms | 1.127 ms |

All rows stayed below the combined `>5 ms` and `>25%` materiality gate. The
largest ratio was the Context fan-out at 1.176×; its 30.290 ms delta did not
approach the ratio threshold. Serialized output sizes matched for every path.

### Focused repeated A/B

The initial query joined each observation to its active source. Bounded stats
made SQLite add a source bloom filter and reproduced a material cold slowdown:
14.469 ms statless versus 21.401 ms analyzed median, +6.932 ms and 1.479×.
Because model history reads no source columns, the production query now uses a
correlated `EXISTS` membership check through the source primary key instead of
an inner join. This expresses the suppression invariant directly and prevents
the inappropriate bloom-filter build.

Eight statless/analyzed pairs rechecked the fixed production endpoint on the
same disposable copy. Each pair deleted `sqlite_stat1`, reloaded the statless
planner, ran the real 24-hour model-history reader on a fresh handle, then
restored stats through production bounded ANALYZE and ran the reader again.

| State | Cold samples (ms) | Median cold | Warm range |
| --- | --- | ---: | ---: |
| Statless | 26.240, 20.677, 17.475, 19.373, 19.194, 21.426, 16.621, 16.500 | 19.283 ms | 0.914–1.255 ms |
| Analyzed | 19.438, 18.441, 19.918, 18.191, 18.763, 16.735, 23.208, 19.064 | 18.914 ms | 1.051–1.398 ms |

Analyzed median was **0.370 ms faster and 0.981× statless**, clearing both
failure conditions. All 16 outputs were exactly 3,984 bytes with identical SHA-256
`71ddfb280272f16af1a85165757fa845adc864e2b4c9cec3dae400a9f09a5a31`.
Focused EQP was byte-identical: observed-time range search, correlated source
primary-key lookup, and GROUP BY temp B-tree in both states.

The final focused pass restored the same 121 `sqlite_stat1` rows and digest,
left WAL at zero, and passed `quick_check`. After all work, frozen source bytes,
mode `0444`, inode, mtime, SHA-256, zero-byte WAL, and 32,768-byte SHM were
unchanged. Only online-backup disposable copies were made writable.

## Merged pending/completed result

**Verdict: PASS — every merged-plan change is conservative and every one-shot
timing flag is cleared by canonical exact-path 8+8 confirmation.**

Attempt 3 used fresh SQLite online backups of the same 13,525,123,072-byte
corpus, pinned at `2026-08-02T19:27:43Z`. The pending copy retained raw model
fallback state. A separate copy ran the production model backfill twice: once
directly and once inside raw/hybrid parity verification. Both processed
4,201,401 observations in 190 chunks, finished `complete`, retained
`raw_pruned=0`, and reported zero missing, mismatched, or extra rollup rows.
The completed copy held 9,734 model hourly rows.

Immediately before each writable audit, the database, WAL, and SHM were mode
`0600`; no database reopened between that permission step and the audit. Full
reports and focused reports used new create-only JSON paths.

### Full matrix and statistics

| State | SQL statements | Timed paths | Exact plans | Changed plans | Plan regressions | One-shot timing flags | `sqlite_stat1` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| Pending/raw | 250 | 42 | 240 | 10 | 0 | 2 | 121 rows, `85ec791dec0c017e487b66e14d11865c83777068322177cb0f1f92df0084fd32` |
| Completed | 16,086 | 42 | 16,076 | 10 | 0 | 4 | 123 rows, `6a85a830891e162de3bd695973b6304e59fa43f2dc0a388f433eef136b6339bc` |

Both states read back `analysis_limit=1000`, retained `sqlite_stat1`, finished
with checkpoint log/checkpointed frames 0/0, and returned `quick_check=ok`.
The pending full run spent 13,501.184 ms inside ANALYZE and 13,512.190 ms in
the complete statistics step. The fresh completed run spent 2,265.271 ms and
2,276.501 ms respectively; an independent durable completed pass also covered
16,086 statements with zero one-shot flags.

Pending changed shapes were project-breakdown timestamp skip-scan bounds
(`2c2a9d2b4307`, four occurrences), a represented-provider source bloom filter
(`516c86b6c2ff`, three), and a raw-source source bloom filter
(`ed67cf161382`, three). Completed changes were the same project skip-scan plus
bounded overview-edge (`27f367add63f`, three) and represented-provider
(`fd841458f23f`, three) source bloom filters. Named searches, SCAN counts, and
temporary ordering counts stayed fixed.

Completed overview still performs distinct leading and trailing half-open raw
edge seeks through `idx_model_observations_observed_provider`. Project-prefix
and running-now reads retain bounded index seeks without SQL temporary sorts.
Pending history retains its correlated active-source `EXISTS` lookup; completed
history instead reads its per-call temp active-source table and excludes keys
in the temp pruned-authority table from its split residual seeks.

### One-shot flags and exact repeated confirmation

The full commands failed closed and persisted all flagged rows before exiting:

| State | Path | Statless cold | Analyzed cold | Delta | Ratio | Statless warm | Analyzed warm |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Pending | Bucket stats, 24h | 93.945 ms | 120.001 ms | +26.056 ms | 1.277x | 1.157 ms | 1.091 ms |
| Pending | Hook breakdown, 90d | 246.410 ms | 310.825 ms | +64.415 ms | 1.261x | 284.188 ms | 280.052 ms |
| Completed | Context fan-out, 30d | 182.136 ms | 240.757 ms | +58.620 ms | 1.322x | 1.083 ms | 2.227 ms |
| Completed | Bucket stats, 24h | 82.121 ms | 121.851 ms | +39.730 ms | 1.484x | 0.914 ms | 1.123 ms |
| Completed | Model overview, 24h | 32.235 ms | 43.430 ms | +11.195 ms | 1.347x | 0.914 ms | 1.477 ms |
| Completed | Bucket stats, 30d | 80.886 ms | 110.444 ms | +29.558 ms | 1.365x | 1.013 ms | 1.365 ms |

Each row then ran as an exact production path on its original pending or
completed copy. Every ordinal alternated statless clear/reload/checkpoint,
fresh cold/warm read, bounded production ANALYZE, and fresh cold/warm read.
All 16 outputs per path had identical canonical bytes and SHA-256:

| State | Path | Statless median | Analyzed median | Delta | Ratio | Bytes | SHA-256 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| Pending | Bucket stats, 24h | 76.331 ms | 76.568 ms | +0.236 ms | 1.003x | 893 | `f757ef9ae2e4fe1c04b92feb962c2cd558685932041fa2fe30454edd7a4f55d3` |
| Pending | Hook breakdown, 90d | 319.082 ms | 301.936 ms | -17.147 ms | 0.946x | 22,995 | `3995579fb4393f7c795112f014f76230c4627a89125e2c2a99c0b42bf133f319` |
| Completed | Context fan-out, 30d | 205.253 ms | 197.567 ms | -7.686 ms | 0.963x | 51,753 | `5c0bee40113666647a4685f9e8cfd23e993f3706e1cec37f1484184b9de8c31b` |
| Completed | Bucket stats, 24h | 82.785 ms | 93.009 ms | +10.224 ms | 1.123x | 893 | `f757ef9ae2e4fe1c04b92feb962c2cd558685932041fa2fe30454edd7a4f55d3` |
| Completed | Model overview, 24h | 31.685 ms | 37.237 ms | +5.553 ms | 1.175x | 5,222 | `7a0e789d7d6d3efdc84ceb5721e04c15d6e4f9b25e1bfbc9b617d4ded4147580` |
| Completed | Bucket stats, 30d | 108.564 ms | 111.085 ms | +2.521 ms | 1.023x | 956 | `481d4ecd269c749adc49b3540989b56217b4cc6f0e0097fb19cc350fca2a9bb1` |

No median crossed both `>5 ms` and `>25%`. Focused EQP comparison found zero
regressions; only two completed model-overview occurrences added the already
audited source bloom filter. Final focused passes restored the exact state-
specific statistics digest, checkpointed 0/0, and returned `quick_check=ok`.

### Completed model release gates and source integrity

| Query | Window | Cold | Warm | Bytes | SHA-256 |
| --- | --- | ---: | ---: | ---: | --- |
| Model overview | 30d | 307.778 ms | 1.172 ms | 12,761 | `ec00856281427f5f619a49ef19ba9f4da599301c64a8a8f8b1f5c6657d4ef601` |
| Model history | 30d | 281.996 ms | 1.149 ms | 5,151 | `72c50ada905adb8aa26a46e095a4e85a5404f73308b381436f9d1652a4d33efe` |
| Model overview | 90d | 296.495 ms | 1.520 ms | 17,131 | `c9e291097a69fb5d18ce0d06a7ef15225fd2cd7ae3d440ced3627f7b746a0ee5` |
| Model history | 90d | 299.793 ms | 1.260 ms | 14,771 | `d20200a035a7f988459a4616daa4165ad26c88ab87cc2708a3f2ea2055c36933` |

All four completed reads remain below 500 ms and match the established merged
hashes. The immutable source remained mode `0444`, inode 21,943,763, size
13,525,123,072, and SHA-256
`c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e`.
Its zero-byte WAL remained inode 21,943,766 with the empty-file SHA-256; its
32,768-byte SHM remained inode 21,943,767 with SHA-256
`fd4c9fda9cd3f9ae7c962b0ddf37232294d55580e1aa165aa06129b8549389eb`.
