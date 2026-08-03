# Widget query timing measurement

This acceptance record freezes the BEFORE query costs and corpus identity, then
records the post-A/B gate and corrected-volume rollup sizing evidence.

## Frozen corpus

The source was the live Linux database at
`/home/mamba/.local/share/com.quilltoolkit.app/usage.db`. SQLite's online backup
API copied one consistent snapshot to this stable, untracked location:

`/home/mamba/.local/share/com.quilltoolkit.app/benchmark-corpora/widget-query-perf/usage-2026-08-02.db`

The corpus is local data and is never committed. Its pinned identity is:

| Property | Value |
| --- | --- |
| Captured | `2026-08-02T19:27:43Z` |
| Bytes | `13,525,123,072` |
| SHA-256 | `c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e` |
| Permissions | `0444` |
| SQLite | `3.45.0` |
| Schema version | `37` |
| Pages | `3,302,032 × 4,096` bytes |
| Freelist | `61,175` pages |
| Validation | `PRAGMA quick_check = ok` |

The live snapshot already carried additive schema version 37. These BEFORE
measurements execute the branch's raw pre-rollup implementations, so no rollup
table contributes to a recorded result.

## Protocol

`widget_query_perf_spike` is a maintainer-only Cargo binary. `freeze` opens the
source read-only, refuses to overwrite its destination, uses SQLite online
backup, validates the copy, and removes destination write permission. `measure`
refuses a writable corpus and opens a fresh read-only `Storage` for each query,
which bypasses prior app-level cache state while retaining production SQL and
Rust post-processing. The post-A/B harness immediately repeats each operation
on that same handle as its controlled warm sample, then runs the exact 30-day
Usage/Charts/Context backend fan-outs cold and warm on fresh per-view handles.

Every query uses one thread-local pinned clock. Cold means its first in-process
call with app caches bypassed. OS page cache is uncontrolled; the fixed order is
24h, then 30d, then 90d, so costs need not grow monotonically. The binary was
built with `--release` on Linux 6.17.0, an AMD Ryzen Threadripper 3970X with 64
logical CPUs and 125 GiB RAM.

```bash
cargo run --release --bin widget_query_perf_spike -- freeze SOURCE DEST
cargo run --release --bin widget_query_perf_spike -- measure CORPUS 2026-08-02T19:27:43Z
cargo run --release --bin widget_query_perf_spike -- measure-session-breakdown CORPUS 2026-08-02T19:27:43Z 10
```

Pinned half-open windows:

| Window | Start | End |
| --- | --- | --- |
| 24h | `2026-08-01T19:27:43Z` | `2026-08-02T19:27:43Z` |
| 30d | `2026-07-03T19:27:43Z` | `2026-08-02T19:27:43Z` |
| 90d | `2026-05-04T19:27:43Z` | `2026-08-02T19:27:43Z` |

The matrix covers the rollup candidates, runtime path, slice-E cleanups,
range-changing Trends query, five initial breakdown commands, and both 013
cached-only endpoints. `Output bytes` validates each call returned a
serializable result rather than failing silently.

## BEFORE results

| Query | Window | Cold | Output bytes |
| --- | --- | ---: | ---: |
| `get_model_usage_overview` | 24h | 250.355 ms | 5,200 |
| `get_model_history` | 24h | 12.499 ms | 3,963 |
| `get_token_history` | 24h | 1.438 ms | 42,027 |
| `get_llm_runtime_stats` | 24h | 277.845 ms | 219 |
| `get_code_stats` | 24h | 42.702 ms | 773 |
| `get_code_stats_history` | 24h | 2.157 ms | 9,181 |
| `get_host_breakdown` | 24h | 11.400 ms | 113 |
| `get_project_breakdown` | 24h | 6.347 ms | 1,647 |
| `get_session_breakdown` | 24h | 46.790 ms | 8,069 |
| `get_skill_breakdown` | 24h | 0.700 ms | 265 |
| `get_hook_breakdown` | 24h | 218.681 ms | 15,793 |
| `get_all_bucket_stats` | 24h | 169.491 ms | 893 |
| `get_context_savings_analytics` | 24h | 41.218 ms | 47,587 |
| `get_model_usage_overview` | 30d | 71,959.774 ms | 12,739 |
| `get_model_history` | 30d | 6,138.933 ms | 5,129 |
| `get_token_history` | 30d | 9.976 ms | 111,291 |
| `get_llm_runtime_stats` | 30d | 9,948.795 ms | 261 |
| `get_code_stats` | 30d | 2,471.927 ms | 1,069 |
| `get_code_stats_history` | 30d | 60.758 ms | 3,091 |
| `get_host_breakdown` | 30d | 1.222 ms | 115 |
| `get_project_breakdown` | 30d | 3.443 ms | 2,717 |
| `get_session_breakdown` | 30d | 883.987 ms | 61,698 |
| `get_skill_breakdown` | 30d | 15.879 ms | 8,416 |
| `get_hook_breakdown` | 30d | 2,550.247 ms | 23,280 |
| `get_all_bucket_stats` | 30d | 164.779 ms | 956 |
| `get_context_savings_analytics` | 30d | 500.936 ms | 51,753 |
| `get_model_usage_overview` | 90d | 44,401.936 ms | 17,109 |
| `get_model_history` | 90d | 6,268.581 ms | 14,749 |
| `get_token_history` | 90d | 3.291 ms | 215,443 |
| `get_llm_runtime_stats` | 90d | 3,856.366 ms | 261 |
| `get_code_stats` | 90d | 584.547 ms | 1,073 |
| `get_code_stats_history` | 90d | 89.693 ms | 9,002 |
| `get_host_breakdown` | 90d | 1.253 ms | 115 |
| `get_project_breakdown` | 90d | 4.222 ms | 2,717 |
| `get_session_breakdown` | 90d | 47.091 ms | 61,698 |
| `get_skill_breakdown` | 90d | 39.214 ms | 14,143 |
| `get_hook_breakdown` | 90d | 312.579 ms | 22,995 |
| `get_all_bucket_stats` | 90d | 157.313 ms | 958 |
| `get_context_savings_analytics` | 90d | 574.557 ms | 270,986 |

The 013 cached-only endpoints therefore cost 157-169 ms for
`get_all_bucket_stats` and 41-575 ms for `get_context_savings_analytics` on
their first app-cache-bypassed call. Later after-measurements must use the same
corpus, endpoint, query order, release profile, and OS-page-cache caveat.

## Model ingest fold overhead

The model-rollup ingest acceptance benchmark compares the production source
replacement path against the same transaction with only its rollup fold
disabled. Parsing, fingerprinting, storage initialization, and warm-up are
outside the timed region. Each release-mode sample replaces one source with a
6,000-row burst concentrated into ten minutes and two model buckets.

```bash
cargo test --release --lib model_hourly_ingest_fold_burst_p95_stays_within_budget -- --ignored --nocapture
```

| Variant | Rows/batch | Samples | p95 |
| --- | ---: | ---: | ---: |
| Raw replacement control | 6,000 | 25 | 127.303 ms |
| Replacement with fold | 6,000 | 25 | 134.070 ms |

Fold overhead is **5.316%**, within the required maximum of 10%.

## Model rollup backfill

The production model target ran on a worker-owned disposable copy. The frozen
source and pristine copy both matched the pinned 13,525,123,072-byte SHA-256
before mutation; the source remained mode `0444` and matched again after the
copy, WAL, and SHM were removed.

```bash
cargo run --release --bin widget_query_perf_spike -- backfill-model COPY
```

| Measure | Result |
| --- | ---: |
| Raw observations backfilled | 4,201,401 / 4,201,401 |
| First committed chunk | 5,239 rows |
| First terminal | Interrupted |
| Resume bookmark | 1,774,954,799,999 ms |
| Final terminal / status | Completed / complete |
| Chunks | 190 |
| End-to-end backfill | 15,121.980 ms |
| Raw-to-rollup missing or mismatched groups | 0 |
| Extra unpruned groups | 0 |
| `raw_pruned=1` rows before / after | 0 / 0 |
| Max WAL after a checkpoint | 0 bytes |
| WAL at finish | present, 0 bytes |
| SHM while final connection was open | present |
| Max progress-to-progress interval | 383.072 ms |

The 383.072 ms interval spans off-permit boundary selection and disk preflight,
the permit transaction, and the post-permit TRUNCATE checkpoint; it is not a
lease or transaction duration. The focused fair-gate regression separately
proves a queued maintenance writer acquires within the shared 250 ms chunk
bound. The empty-database regression completed zero-of-zero in one committed
terminal chunk in 28.684 ms, below its one-second small-database ceiling.

## Model rollup consistency

The Family 1 equality leg attaches the frozen source immutable/read-only,
extracts only its pinned 90-day model rows and matching registry sources into
a disposable database, then runs production backfill and requires exact
normalized equality at all three acceptance windows:

```bash
cargo run --release --bin widget_query_perf_spike -- verify-model-rollup-derived SOURCE FIXTURE 2026-08-02T19:27:43Z
```

| Measure | Result |
| --- | ---: |
| Fixture sources | 4,392 |
| Fixture observations | 4,100,262 |
| Fixture bytes before backfill | 5,991,608,320 |
| `PRAGMA quick_check` | `ok` |
| First committed chunk | 5,124 rows |
| Resume bookmark | 1,778,205,599,999 ms |
| Final terminal / status | Completed / complete |
| Chunks | 172 |
| End-to-end backfill | 14,554.554 ms |
| Raw-to-rollup missing or mismatched groups | 0 |
| Extra unpruned groups | 0 |
| `raw_pruned=1` rows before / after | 0 / 0 |

| Window | Overview bytes | Overview SHA-256 | History bytes | History SHA-256 | Exact |
| --- | ---: | --- | ---: | --- | --- |
| 24h | 5,131 | `1291275007a478751980859e9f20d6dfcd2d23876efc9692261a86d338a06c69` | 3,963 | `049126eccc9019ed37e157f53ea00d88f59eb3f8f76ee5a863ca786e33c5cda1` | yes |
| 30d | 12,670 | `fb50579fa9c6f866bb9b1c3311d9cb86d790a91ecd60792557be763a7c8c679d` | 5,129 | `d2b0463451f9e1f098a8ed0a7a5e99d0ef8330d374a217032c2880d42416fe8c` | yes |
| 90d | 17,040 | `ded5f5ff9020dc308015c16159f4456fefcef514a550b786628b3fe787b3afd9` | 14,749 | `ef4e4d7a789a027912bcf94ce5babc192aac97c48b2d57d5b74a16529acca662` | yes |

The source remained 13,525,123,072 bytes, mode `0444`, and SHA-256
`c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e`.
Its read-only zero-byte WAL and 32,768-byte SHM retained their baseline modes
and mtimes. The validated 5.99 GB derived fixture was deleted after the run.

### Post-source-admission sizing and consistency rerun

This rerun followed `quill-xnb` commit `689ae74`, which independently re-admits
retained model inventory at startup and feeds periodic rescans into both
analytics queues. It used the same immutable source, pinned endpoint, bounded
90-day extraction, production interrupted/resumed backfill, raw-refold check,
and raw/hybrid query legs as the Family 1 run above.

The immutable source still contains exactly the prior run's 4,100,262 bounded
observations and 4,392 sources. Provider and rollup density are:

| Provider | Observations | Sources | Source-hours | Rollup rows | Obs/rollup | p50 | p95 | Max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Claude | 179,411 | 2,150 | 3,261 | 3,272 | 54.83 | 34 | 183 | 545 |
| Codex | 3,920,851 | 2,242 | 3,826 | 4,016 | 976.31 | 65 | 7,242 | 15,845 |
| Total | 4,100,262 | 4,392 | 7,087 | 7,288 | 562.60 | — | — | 15,845 |

The raw identity count is also 4,100,262, so the unique
`(provider, source_key, source_record_key)` contract has no duplicates. One
rollup source-hour carries 1.003 models on average for Claude and 1.050 for
Codex; each provider's p50 and p95 are both one model per source-hour.

The measured Codex post-`2026-07-28T00:00:00Z` volume is 336,842 observations
across 153 sources. Its daily admission shape remains:

| UTC day | Observations | Sources | Rollup rows | Obs/rollup |
| --- | ---: | ---: | ---: | ---: |
| 2026-07-28 | 321,392 | 114 | 227 | 1,415.82 |
| 2026-07-29 | 5,840 | 22 | 83 | 70.36 |
| 2026-07-30 | 1,472 | 10 | 29 | 50.76 |
| 2026-07-31 | 2,865 | 8 | 41 | 69.88 |
| 2026-08-01 | 1,092 | 3 | 13 | 84.00 |
| 2026-08-02 partial | 4,181 | 12 | 45 | 92.91 |

Only five Codex registry sources, carrying 974 observations, have a successful
reconciliation at or after `2026-08-02T18:00:00Z`; the latest is
`2026-08-02T19:15:00.704Z`. The fix commit is timestamped
`2026-08-02T19:17:46Z`, ten minutes before corpus capture. This corpus therefore
does not prove the fully corrected retained-source volume promised by the
post-`quill-xnb` acceptance task. The completed rerun below freezes a distinct
snapshot after reconciliation completion.

The existing burst envelope remains conservative for observed row counts. The
90-day Codex peak is 865,263 observations from 73 sources on 2026-07-27, which
folds to 129 rows (6,707.47 observations per rollup row). Physical sizing,
however, invalidates the plan's 350-byte estimate:

| B-tree set | Table bytes | Index bytes | Total bytes |
| --- | ---: | ---: | ---: |
| Raw observations | 2,414,886,912 | 3,569,659,904 | 5,984,546,816 |
| Model hourly rollup | 2,781,184 | 4,661,248 | 7,442,432 |

The bounded fixture gains 7,430,144 bytes after backfill. Raw b-trees consume
1,459.55 bytes per observation; the rollup consumes 1,021.19 bytes per row
(381.61 table + 639.58 indexes), while 562.60:1 row compression produces
804.11:1 physical compression. At this measured occupancy, 1.8 million annual
rollup rows project to 1.84 GB, not the plan's former ~650 MB. The
corrected-volume rerun below carries that input into the final budget.

Two independently derived fixtures produced the same backfill structure:

| Measure | Fixture A | Fixture B |
| --- | ---: | ---: |
| Sources / observations | 4,392 / 4,100,262 | 4,392 / 4,100,262 |
| First chunk / resume bookmark | 5,124 / 1,778,205,599,999 ms | 5,124 / 1,778,205,599,999 ms |
| Chunks | 172 | 172 |
| Backfill elapsed | 12,477.102 ms | 12,481.933 ms |
| Max progress interval | 337.900 ms | 341.039 ms |
| Max WAL after checkpoint / finish | 0 / 0 bytes | 0 / 0 bytes |
| Missing-or-mismatched / extra rows | 0 / 0 | 0 / 0 |
| Terminal / committed status | Completed / complete | Completed / complete |

Both returned `PRAGMA quick_check = ok` and preserved zero `raw_pruned=1`
rows. Progress intervals include boundary selection, preflight, the permit-held
transaction, and checkpoint work; they are not transaction-hold measurements.

The first comparison exposed a verifier-only reproducibility defect. Fresh
scratch databases differed solely at canonical JSON path
`/backfill/updatedAt` because migration 28 stamps `model_backfill_state` at
fixture creation. Aligning only that database value made every overview digest
equal. The study normalizer now removes only this lifecycle timestamp in
addition to `buildingIndex`; semantic fields remain covered. With the two
fixtures' distinct original timestamps restored, both independently produced:

| Window | Overview bytes | Overview SHA-256 | History bytes | History SHA-256 | Exact |
| --- | ---: | --- | ---: | --- | --- |
| 24h | 5,087 | `40615ee37d93f5d9b9bee0ae3da327fa1ba67748f20a3beb246b67ba9dd3081d` | 3,963 | `049126eccc9019ed37e157f53ea00d88f59eb3f8f76ee5a863ca786e33c5cda1` | yes |
| 30d | 12,626 | `4a50ffec366608c025d60fc6c30b28bfe729afc6e662ac41f2555f0360698533` | 5,129 | `d2b0463451f9e1f098a8ed0a7a5e99d0ef8330d374a217032c2880d42416fe8c` | yes |
| 90d | 16,996 | `bf9d6d001fde3aa03b0f6c6f4e5367703a07e9c1a78f954bc54d3a13eb285bad` | 14,749 | `ef4e4d7a789a027912bcf94ce5babc192aac97c48b2d57d5b74a16529acca662` | yes |

The source remained 13,525,123,072 bytes, mode `0444`, inode `21943763`,
mtime `1785698785`, ctime `1785698835`, and canonical SHA-256
`c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e`.
The 32,768-byte SHM stayed mode `0444`, inode `21943767`, with mtime/ctime
`1785699012.9498673700`; the zero-byte WAL stayed mode `0444`, inode
`21943766`, with mtime/ctime `1785699012.9481790910`. Both validated
5,999,038,464-byte fixture databases and their dedicated temp directory were
deleted after the stable-digest rerun; no fixture sidecars remained at cleanup.

### Corrected-volume corpus and final sizing rerun

The follow-up corpus was frozen only after the production reconciliation path
proved current retained inventory parity. At
`2026-08-03T06:28:00.031Z`, two identical metadata-only inventories bracketed
one read-only registry snapshot: all 7,181 current sources were admitted, none
had a size/mtime/status mismatch, and durable backfill state was complete with
both roots resolved and no failures. The registry contained 460 additional
historical paths whose files had since been removed; live reconciliation does
not prune them.

The release `freeze` command immediately copied the WAL-consistent live
database to this new stable, untracked path without replacing the original
BEFORE corpus:

`/home/mamba/.local/share/com.quilltoolkit.app/benchmark-corpora/widget-query-perf/usage-2026-08-03.db`

| Property | Corrected corpus |
| --- | --- |
| Captured | `2026-08-03T06:28:00.031Z` |
| Bytes | `16,738,598,912` |
| SHA-256 | `782a4d5553f9271b13684d86870c4000dcbcded81c25f7f99b4e3d22e3dfdacc` |
| Permissions | `0444` |
| SQLite | `3.45.1` |
| Schema version | `37` |
| Pages | `4,086,572 × 4,096` bytes |
| Freelist | `0` pages |
| Validation | `PRAGMA quick_check = ok` |
| Sidecars after close | WAL absent; SHM absent |

The original `usage-2026-08-02.db` remains the immutable BEFORE and post-A/B
timing baseline. It retained its 13,525,123,072-byte size, mode `0444`, inode,
timestamps, sidecars, and canonical SHA-256 throughout this rerun. Results in
this subsection use only the corrected corpus and the later pinned endpoint.

The corrected corpus has 217,361 Claude and 6,236,478 Codex observations. Its
Codex volume at or after `2026-07-28T00:00:00Z` is 2,551,330 observations
across 894 sources, versus 336,842 across 153 in the original corpus. That is
2,214,488 more observations and 741 more sources: 7.57× and 5.84× the old
totals, respectively. The post-fix daily Codex density is:

| UTC day | Observations | Sources | Source-hours | Rollup rows | Obs/rollup |
| --- | ---: | ---: | ---: | ---: | ---: |
| 2026-07-28 | 793,785 | 257 | 380 | 407 | 1,950.33 |
| 2026-07-29 | 1,465,412 | 234 | 347 | 371 | 3,949.90 |
| 2026-07-30 | 87,525 | 43 | 85 | 89 | 983.43 |
| 2026-07-31 | 133,890 | 87 | 148 | 161 | 831.61 |
| 2026-08-01 | 11,683 | 43 | 60 | 66 | 177.02 |
| 2026-08-02 | 44,261 | 186 | 282 | 300 | 147.54 |
| 2026-08-03 partial | 14,774 | 74 | 112 | 121 | 122.10 |

The same bounded derivation and production interrupted/resumed backfill ran
twice from scratch:

```bash
cargo run --release --bin widget_query_perf_spike -- verify-model-rollup-derived SOURCE FIXTURE 2026-08-03T06:28:00.031Z
```

| Measure | Fixture A | Fixture B |
| --- | ---: | ---: |
| Sources / observations | 5,737 / 6,352,310 | 5,737 / 6,352,310 |
| Bytes before backfill | 9,281,732,608 | 9,281,732,608 |
| First chunk / resume bookmark | 5,194 / 1,778,209,199,999 ms | 5,194 / 1,778,209,199,999 ms |
| Chunks | 219 | 219 |
| Backfill elapsed | 20,373.805 ms | 18,532.347 ms |
| Max progress interval | 748.524 ms | 722.642 ms |
| Max WAL after checkpoint / finish | 0 / 0 bytes | 0 / 0 bytes |
| Missing-or-mismatched / extra rows | 0 / 0 | 0 / 0 |
| Terminal / committed status | Completed / complete | Completed / complete |

Both fixtures returned `PRAGMA quick_check = ok`, preserved zero
`raw_pruned=1` rows, and produced the same exact normalized raw/hybrid results:

| Window | Overview bytes | Overview SHA-256 | History bytes | History SHA-256 | Exact |
| --- | ---: | --- | ---: | --- | --- |
| 24h | 5,416 | `caa665c2c73d31bb2fd69ad372875a1030f9a7ac8602f75aa76bed5fd6609ca9` | 4,228 | `c371b22c4e11d95117216552cb837047457ffe0b040a8e25bd2e9a2efc10c0b4` | yes |
| 30d | 12,921 | `d502ce428a0e206487d7fa26dff26f77f4f96872bbd008341f88d69aa063e51f` | 5,419 | `df8efd2a8487e1587d6b3870745e7fe7adf23526fbdeaa94e44b5f05b9ca6bfa` | yes |
| 90d | 17,510 | `edeeefbd5b2536876e8c6e0d49cfbade6e43689498399cd58b17e4e2da590b83` | 15,523 | `09caa09af9c2c8815bccb81e43d1c60a52b293483b69f9cd55f0671e14c4173e` | yes |

The bounded provider/source-hour density is:

| Provider | Observations | Sources | Source-hours | Rollup rows | Obs/rollup | p50 | p95 | Max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Claude | 217,361 | 2,762 | 4,000 | 4,014 | 54.15 | 35 | 177 | 545 |
| Codex | 6,134,949 | 2,975 | 4,806 | 5,078 | 1,208.14 | 73 | 10,719 | 18,083 |
| Total | 6,352,310 | 5,737 | 8,806 | 9,092 | 698.67 | — | — | 18,083 |

Claude averages 1.0035 models per source-hour, with p50/p95 both one.
Codex averages 1.0566, with p50 one and p95 two. Raw identity count is also
6,352,310, so the provider/source/record uniqueness contract has no
duplicates.

Physical occupancy changed only slightly despite the corrected observation
volume:

| B-tree set | Table bytes | Index bytes | Total bytes |
| --- | ---: | ---: | ---: |
| Raw observations | 3,741,335,552 | 5,531,234,304 | 9,272,569,856 |
| Model hourly rollup | 3,485,696 | 5,890,048 | 9,375,744 |

The fixture gained 9,363,456 file bytes after backfill. Raw B-trees consume
1,459.72 bytes per observation; the rollup consumes 1,031.21 bytes per row.
The measured 698.67:1 row compression yields 989.00:1 physical compression.
At this occupancy, the plan's conservative 1.8 million annual rollup rows
require 1.856 GB (1.729 GiB). The corrected peak day has 1,476,108 combined
observations but only 572 combined rollup rows, so the 5,000-row/day rollup
envelope remains 8.74× above observed density.

At each verifier finish the WAL was present at zero bytes and the SHM existed
only while its final connection was open. After both read-only validation
handles closed, the fixture sidecars were absent. The two validated fixture
databases and their dedicated temporary directory were then deleted; the two
immutable corpora retained no open handles.

## Runtime ingest fold overhead

The runtime-rollup benchmark compares production transcript replacement with
the same transaction after disabling only runtime refold. Parsing, snapshot
construction, storage initialization, and warm-up stay outside timing. Each
release-mode sample replaces one source with 6,000 persisted session events;
short burst gaps include periodic logical-turn closures and tool loops.

```bash
cargo test --release --lib runtime_hourly_ingest_fold_burst_p95_stays_within_budget -- --ignored --nocapture
```

| Variant | Rows/batch | Samples | p95 |
| --- | ---: | ---: | ---: |
| Raw replacement control | 6,000 | 25 | 88.230 ms |
| Replacement with runtime fold | 6,000 | 25 | 96.017 ms |

Runtime fold overhead is **8.825%**, within the required maximum of 10%.

## Runtime hybrid read

The runtime rollup was backfilled on a worker-owned disposable copy whose
pre-mutation SHA-256 matched the frozen source. Source preparation ran outside
the ingest permit; each compact source commit used the shared transaction,
checkpoint, and 250 ms deadline.

```bash
cargo run --release --bin widget_query_perf_spike -- backfill-runtime COPY
cargo run --release --bin widget_query_perf_spike -- measure-runtime COPY 2026-08-02T19:27:43Z
```

| Measure | Result |
| --- | ---: |
| Source-keyed rows backfilled | 2,878,277 |
| Backfill elapsed | 134,006.999 ms |
| `get_llm_runtime_stats` cold @90d | 81.977 ms |
| Output bytes | 252 |

The final read is below the 200 ms budget. A rejected first tail plan used the
global earliest open-turn timestamp and took 11,509.582 ms because it emitted
1,556,010 raw rows. The final plan scans 5,129 active open states and performs
one packed last-event seek per native chain through
`idx_se_provider_chain_timestamp`; a direct diagnostic measured that row fetch
at 52.816 ms before the production endpoint measurement.

## Independent runtime parity

The parity verifier implements logical-turn walking and output shaping without
calling production fold or shape helpers. It independently pins native-chain
identity, five-minute idle closure, the six-hour tool-loop ceiling, start-hour
attribution, provider-qualified sessions, parent-only filtering, and trailing
tool-use realization against the fixed endpoint.

It attaches the frozen source immutable/read-only, creates a fresh schema, and
copies only active events needed by the 90-day window. For each source it also
copies the exact logical-turn prefix crossing the floored boundary, preventing
a truncated fixture from inventing a new turn start.

```bash
cargo run --release --bin widget_query_perf_spike -- verify-runtime-parity-derived SOURCE FIXTURE 2026-08-02T19:27:43Z
```

| Measure | Result |
| --- | ---: |
| Fixture sources | 5,130 |
| Fixture events | 2,339,428 |
| Fixture bytes before backfill | 3,005,239,296 |
| Estimated required / available bytes | 2,437,599,232 / 651,293,700,096 |
| Backfill rows | 2,339,428 / 2,339,428 |
| Backfill chunks | 5,130 |
| Backfill elapsed | 101,237.202 ms |
| Runtime rollup / open-state rows | 2,148 / 5,130 |
| `PRAGMA quick_check` | `ok` |

Every integer field matches exactly. Runtime totals, averages, and all seven
sparkline buckets first match within the established `1e-6`-second tolerance,
then match after conversion to integer microseconds for the recorded digest.
Each completed production read also matched an immediate repeated read.

| Window | Scope | Runtime seconds | Turns | Sessions | Normalized SHA-256 |
| --- | --- | ---: | ---: | ---: | --- |
| 24h | all | 273,033.051 | 402 | 31 | `1917cbc863e27049e976ac6b3fcf846924babf805b326462bd2903c4fda7c8c1` |
| 24h | parent only | 143,496.212 | 117 | 31 | `51a47edc5e448d464a234d933ea5a58144f92464c44c5934f2f7c69f91d4c548` |
| 30d | all | 4,951,227.057 | 7,262 | 540 | `4fe8abb113a20ff263e3f9a40eb7b7904b17b0d0e3c12bd2b052e42f7f37020d` |
| 30d | parent only | 2,357,889.470 | 1,901 | 540 | `f880296c7cbaef2b4c60cf850c6d063380341b79093ef476ae0831cb519c5d93` |
| 90d | all | 5,572,381.772 | 8,114 | 924 | `3bc9c85a74f10044f60aebebc1d29d838fa120c130de6f14531fdc59cd31d137` |
| 90d | parent only | 2,960,171.796 | 2,686 | 924 | `f2ba4bdc8a4f090aa423fd10006ce0c8f93c4afcbe6e2fa1c0d4883719a9501a` |

Before and after the run, the frozen source remained 13,525,123,072 bytes,
mode `0444`, with unchanged inode, mtime, and canonical SHA-256
`c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e`.
Its zero-byte WAL and 32,768-byte SHM retained identical metadata. The only
temporary file was the validated 3,010,273,280-byte fixture; it and its
`mktemp` directory were removed after `quick_check=ok` and parity completion.

## Post-A/B interim re-measurement

The gate used one disposable copy for both authoritative rollups and every
measurement. Before mutation, source and copy were each 13,525,123,072 bytes
with SHA-256
`c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e`;
the source was mode `0444`. Available space was 684,695,068,672 bytes.

The release binary was built before corpus work. Command order was:

```bash
cargo build --release --bin widget_query_perf_spike
cp /home/mamba/.local/share/com.quilltoolkit.app/benchmark-corpora/widget-query-perf/usage-2026-08-02.db ../widget-query-perf-post-ab.db
chmod u+w ../widget-query-perf-post-ab.db
./target/release/widget_query_perf_spike backfill-model ../widget-query-perf-post-ab.db
./target/release/widget_query_perf_spike backfill-runtime ../widget-query-perf-post-ab.db
chmod 0444 ../widget-query-perf-post-ab.db
./target/release/widget_query_perf_spike measure ../widget-query-perf-post-ab.db 2026-08-02T19:27:43Z
./target/release/widget_query_perf_spike diagnose-model ../widget-query-perf-post-ab.db 2026-08-02T19:27:43Z
```

The model backfill completed 4,201,401/4,201,401 raw observations in 190
chunks after the required first-chunk interrupt/resume, in 42,049.203 ms. It
reported zero missing/mismatched groups, zero extra groups, and `raw_pruned=1`
rows unchanged at zero. Runtime completed 2,878,277/2,878,277 source-keyed rows
in 184,373.483 ms. Final `rollup_meta` state was model `complete`, runtime
`complete`, generation 7,112; model/runtime rollups contained 9,734/2,674 rows,
both with zero `raw_pruned=1` rows. `PRAGMA quick_check` returned `ok`.

The measurement ran once, after chmod `0444` and removal of zero-byte WAL/SHM
sidecars. It used SQLite 3.45.0, Linux 6.17.0-29-generic, and the same 64-CPU
AMD Ryzen Threadripper 3970X host. OS page cache remained uncontrolled. Cold
still means first call on a fresh app-cache-empty `Storage`; warm is its
immediate repeat. No anomalous endpoint was rerun after diagnosis established
real residual raw work rather than a measurement-condition artifact.

### Interim per-query results

Query order remained the frozen BEFORE order. BEFORE has no same-corpus warm
column because the original harness retained cold results only.

| Query | Window | BEFORE cold | Post-A/B cold | Post-A/B warm | Output bytes |
| --- | --- | ---: | ---: | ---: | ---: |
| `get_model_usage_overview` | 24h | 250.355 ms | 57.969 ms | 1.027 ms | 5,222 |
| `get_model_history` | 24h | 12.499 ms | 18.082 ms | 0.941 ms | 3,985 |
| `get_token_history` | 24h | 1.438 ms | 0.165 ms | 0.132 ms | 42,027 |
| `get_llm_runtime_stats` | 24h | 277.845 ms | 4.222 ms | 3.992 ms | 201 |
| `get_code_stats` | 24h | 42.702 ms | 1.696 ms | 1.306 ms | 773 |
| `get_code_stats_history` | 24h | 2.157 ms | 1.460 ms | 1.440 ms | 9,181 |
| `get_host_breakdown` | 24h | 11.400 ms | 0.596 ms | 0.465 ms | 113 |
| `get_project_breakdown` | 24h | 6.347 ms | 0.635 ms | 0.547 ms | 1,647 |
| `get_session_breakdown` | 24h | 46.790 ms | 4.067 ms | 3.515 ms | 8,069 |
| `get_skill_breakdown` | 24h | 0.700 ms | 0.113 ms | 0.082 ms | 265 |
| `get_hook_breakdown` | 24h | 218.681 ms | 25.181 ms | 28.513 ms | 15,793 |
| `get_all_bucket_stats` | 24h | 169.491 ms | 130.128 ms | 0.233 ms | 893 |
| `get_context_savings_analytics` | 24h | 41.218 ms | 15.924 ms | 0.058 ms | 47,587 |
| `get_model_usage_overview` | 30d | 71,959.774 ms | 19,637.751 ms | 1.089 ms | 12,761 |
| `get_model_history` | 30d | 6,138.933 ms | 1,038.036 ms | 1.033 ms | 5,151 |
| `get_token_history` | 30d | 9.976 ms | 1.759 ms | 1.365 ms | 111,291 |
| `get_llm_runtime_stats` | 30d | 9,948.795 ms | 47.329 ms | 49.221 ms | 246 |
| `get_code_stats` | 30d | 2,471.927 ms | 68.932 ms | 64.413 ms | 1,069 |
| `get_code_stats_history` | 30d | 60.758 ms | 59.368 ms | 60.476 ms | 3,091 |
| `get_host_breakdown` | 30d | 1.222 ms | 1.237 ms | 1.042 ms | 115 |
| `get_project_breakdown` | 30d | 3.443 ms | 3.477 ms | 3.317 ms | 2,717 |
| `get_session_breakdown` | 30d | 883.987 ms | 42.849 ms | 42.764 ms | 61,698 |
| `get_skill_breakdown` | 30d | 15.879 ms | 1.486 ms | 1.288 ms | 8,416 |
| `get_hook_breakdown` | 30d | 2,550.247 ms | 251.250 ms | 245.009 ms | 23,280 |
| `get_all_bucket_stats` | 30d | 164.779 ms | 118.823 ms | 0.221 ms | 956 |
| `get_context_savings_analytics` | 30d | 500.936 ms | 232.281 ms | 0.078 ms | 51,753 |
| `get_model_usage_overview` | 90d | 44,401.936 ms | 19,745.694 ms | 1.172 ms | 17,131 |
| `get_model_history` | 90d | 6,268.581 ms | 1,062.273 ms | 1.118 ms | 14,771 |
| `get_token_history` | 90d | 3.291 ms | 2.116 ms | 1.914 ms | 215,443 |
| `get_llm_runtime_stats` | 90d | 3,856.366 ms | 60.310 ms | 66.898 ms | 252 |
| `get_code_stats` | 90d | 584.547 ms | 78.361 ms | 79.498 ms | 1,073 |
| `get_code_stats_history` | 90d | 89.693 ms | 80.064 ms | 78.423 ms | 9,002 |
| `get_host_breakdown` | 90d | 1.253 ms | 1.305 ms | 1.084 ms | 115 |
| `get_project_breakdown` | 90d | 4.222 ms | 3.557 ms | 3.364 ms | 2,717 |
| `get_session_breakdown` | 90d | 47.091 ms | 42.784 ms | 44.645 ms | 61,698 |
| `get_skill_breakdown` | 90d | 39.214 ms | 3.890 ms | 3.522 ms | 14,143 |
| `get_hook_breakdown` | 90d | 312.579 ms | 266.690 ms | 280.957 ms | 22,995 |
| `get_all_bucket_stats` | 90d | 157.313 ms | 137.938 ms | 0.240 ms | 958 |
| `get_context_savings_analytics` | 90d | 574.557 ms | 417.311 ms | 0.136 ms | 270,986 |

Output changes on model/runtime rows are expected: model responses gained the
additive `building_index` field, while runtime uses the approved time-invariant
closed-turn semantics. Cold and warm serialized output matched within each
post-A/B sample.

### Residual model-query diagnosis

Both rollups were complete, but slice A did not eliminate every raw scan. At
30d the corpus held 4,032,736 active raw observations, 5,789 closed-hour rollup
rows, and only 207 raw rows in the intended partial-leading/current-hour tail.
At 90d it held 4,100,262 active raw observations.

`get_model_usage_overview` still computes exact representative projects by
ranking every in-range raw observation. SQLite 3.45.0 used
`idx_model_observations_observed_provider` for the 4,032,736-row range, probed
the source primary key once per row, and built a temporary B-tree for the
window-function order. The stage emitted 471 session rows and alone took
21,014.797 ms. This explains the 19.6-19.7s endpoint results; it is not raw-path
fallback.

Model history's residual branch returned 156,256 rows for daily buckets. Its
plan range-scanned the same observed-time index, ran a correlated primary-key
probe against authoritative rollup rows, then probed each source. Arithmetic
bucket exclusion selects the right boundary hours but does not turn them into
bounded index seeks, explaining the remaining ~1.0s cold cost.

Slice A therefore needs a follow-up before its gate can close: preserve exact
project attribution without all-range raw ranking, and express history/activity
boundary-hour reads as bounded index-seekable ranges. The follow-up must retain
raw-pruned authority and frozen-corpus parity. This measurement task does not
implement that downstream optimization.

### View fan-outs and render boundary

The view harness executes current hook call order on one shared `Storage`. The
Usage list deliberately includes `get_code_stats_history` twice because
`useCodeStats` and the post-runtime `useCodeInsights` request it separately.

| View | Calls in order | Cold backend total | Warm backend total | Output bytes |
| --- | --- | ---: | ---: | ---: |
| Usage 30d | provider series; activity series; token stats; runtime; code stats; code history (code card); context savings; retention policy; session breakdown; project breakdown; token history; code history (insights) | 559.261 ms | 303.173 ms | 236,162 |
| Charts 30d | provider series; code stats; code history; token history; retention policy | 125.473 ms | 124.042 ms | 116,067 |
| Context 30d | context savings | 224.332 ms | 0.210 ms | 51,753 |

These are backend query totals, not frontend render timings. The harness has no
Tauri IPC, React, layout, or paint instrumentation, so it cannot claim the
≤1,200 ms cold-render budget even though every backend subtotal is below it.
The BEFORE harness did not execute exact view fan-outs, so no same-corpus
BEFORE fan-out total exists and none is reconstructed from unrelated rows.

### Interim budget gate

| Measure | Budget | Post-A/B evidence | Gate |
| --- | ---: | ---: | --- |
| Model overview cold @30d | ≤500 ms | 19,637.751 ms | fail |
| Model overview cold @90d | ≤500 ms | 19,745.694 ms | fail |
| Model history cold @30d | ≤500 ms | 1,038.036 ms | fail |
| Model history cold @90d | ≤500 ms | 1,062.273 ms | fail |
| Runtime cold @90d | ≤200 ms | 60.310 ms | pass |
| Session breakdown @30d | ≤300 ms | 42.849 ms | pass |
| Code stats history @30d | ≤300 ms | 59.368 ms | pass |
| Usage cold backend fan-out @30d | render ≤1,200 ms | 559.261 ms backend only | render unproven |
| Charts cold backend fan-out @30d | render ≤1,200 ms | 125.473 ms backend only | render unproven |
| Context cold backend fan-out @30d | render ≤1,200 ms | 224.332 ms backend only | render unproven |
| Fast class under 5s injection | ≤100 ms p95 | slice-C test not implemented | pending |
| Warm regression | no slower | model overview 1.089 ms vs 013's coarse 1 ms; context 0.078 ms vs 238 ms on older corpus | model inconclusive; context pass |

### Slice decisions

- **Slice C — GO, full enumerated scope.** Reader isolation remains required
  for the ≤100 ms contention contract, and cold bucket/context/hook reads still
  spend 119-417 ms on view-serving paths. Preserve all callers listed in the
  plan; prioritize the ≥100 ms paths. Models already use their own reader.
- **Slice D — GO, full correctness scope.** Backend totals fit inside the
  render ceiling, but they do not cover render cost or repeated ingest-driven
  fan-outs. The duplicate Usage history call is present, module-cache survival
  is still unproven, and ≥5s coalescing/range honesty are normative behavior.
- **Slice E — SHRINK.** Keep the targeted `full_input` removal and bounded
  session-subquery cleanup, but defer app-wide bounded `ANALYZE` and its plan
  blast radius. The two hard E endpoints already pass at 59.368 ms and
  42.849 ms; current evidence does not justify changing planner statistics.

These decisions size C/D/E but do not waive slice A's failed cold budgets. The
residual model-query follow-up remains the release gate before feature 020 can
claim its S1 acceptance target.

## Session breakdown candidate pruning

The slice-E session cleanup ran separately from the full matrix so repeated
samples did not pay the unrelated residual model-query cost. Each sample used
a fresh read-only `Storage`, the pinned 30-day endpoint, production SQL, and an
app-cache-empty handle. OS page cache remained uncontrolled.

Source inspection of the legacy SQL showed one range-grouped `tok` CTE feeding
unbounded correlated response-count, latest-activity, latest-project, and
four-table distinct-agent subqueries. SQLite therefore enriched every token
session before the final `ORDER BY ... LIMIT 200`; the frozen BEFORE run cost
883.987 ms and returned 61,698 bytes.

The final plan materializes range-, provider-, and hostname-scoped token groups
and an indexed, range-bounded response maximum, then materializes 200 ranked
candidates before enrichment. Its EQP orders `rankable` while building
`candidates`, scans only `candidates` afterward, and places the response count,
project lookup, and distinct-agent UNION beneath that scan. Raw probes carry
the exact timestamp lower bound. The retained-agent probe uses the overlapping
UTC day because `retention_daily_aggregates` has daily grain.

```text
MATERIALIZE candidates
  MATERIALIZE rankable
    MATERIALIZE tok
    CORRELATED SCALAR SUBQUERY: SEARCH response_times
  USE TEMP B-TREE FOR ORDER BY
SCAN candidates
  CORRELATED SCALAR SUBQUERY: SEARCH response_times
  CORRELATED SCALAR SUBQUERY: SEARCH token_snapshots
  CORRELATED SCALAR SUBQUERY: bounded four-table agent UNION
```

| Samples | Min | Median | p95 | Max | Output bytes | Budget |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | 118.001 ms | 123.797 ms | 141.554 ms | 141.554 ms | 61,698 | ≤300 ms |

All ten serialized outputs were byte-identical. Before and after measurement,
the source remained 13,525,123,072 bytes, mode `0444`, with unchanged inode,
mtime, and canonical SHA-256
`c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e`.
Its existing zero-byte WAL and 32,768-byte SHM also retained identical mode,
inode, mtime, and size; neither sidecar was removed or rewritten.

### Slice E code-stats payload projection

The slice-E reader change preserves the independent-reader snapshot boundary
while replacing unconditional `full_input` projection with:

```sql
CASE WHEN lines_added IS NULL OR lines_removed IS NULL
     THEN full_input
END AS legacy_full_input
```

Both code-stat queries retain `full_input IS NOT NULL` eligibility. Rust uses
persisted counts directly when both are present and invokes the existing
tolerant parser only when the conditional payload is non-NULL. Retained daily
rows and history bucketing are unchanged.

The release harness ran against the frozen corpus with pinned end
`2026-08-02T19:27:43Z`. Each query uses a fresh read-only `Storage`; its paired
cold/warm calls must serialize to exactly equal JSON before timing is accepted.
OS page cache remains uncontrolled. The first full matrix pass measured:

| Query | Window | Cold | Warm | Output bytes |
| --- | --- | ---: | ---: | ---: |
| `get_code_stats` | 24h | 2.443 ms | 2.002 ms | 773 |
| `get_code_stats_history` | 24h | 2.058 ms | 1.999 ms | 9,181 |
| `get_code_stats` | 30d | 57.415 ms | 54.036 ms | 1,069 |
| `get_code_stats_history` | 30d | 52.782 ms | 53.920 ms | 3,091 |
| `get_code_stats` | 90d | 77.453 ms | 86.673 ms | 1,073 |
| `get_code_stats_history` | 90d | 73.094 ms | 71.004 ms | 9,002 |

Three full passes supplied six 30-day history samples (each pass's cold and
controlled warm call): `52.782`, `53.920`, `78.366`, `88.407`, `55.859`, and
`59.876` ms. Nearest-rank p95 is **88.407 ms**, 29.5% of the 300 ms budget.
Every pass returned 3,091 bytes, matching the prior frozen-corpus record.

`EXPLAIN QUERY PLAN` reports
`SEARCH tool_actions USING INDEX idx_tool_actions_category_timestamp
(category=? AND timestamp>?)`. The 30-day window contains 15,408 eligible
rows: 15,407 with both persisted counters and one legacy row. Projection checks
found zero persisted rows with a materialized payload and the one legacy payload
equal to its stored `full_input`.

Before and after measurement, the source remained 13,525,123,072 bytes, mode
`0444`, inode `21943763`, mtime `1785698785`, ctime `1785698835`, and SHA-256
`c86553ab3b0f22e23511dfc43a1f1b9dc9af35ad57f6ae63fcb3de75a673d04e`.
The `-shm` sidecar stayed 32,768 bytes, mode `0444`, inode `21943767`, with
mtime/ctime `1785699012.9498673700`; `-wal` stayed zero bytes, mode `0444`,
inode `21943766`, with mtime/ctime `1785699012.9481790910`.

## Frontend cache and refresh query-window evidence

The slice-D Node mock uses a controllable clock around the production module
store. It mounts two command-and-range keys, emits an invalidation every second,
then removes one subscriber while the next shared fan-out is pending.

```text
npm run test:cached-invoke
[query-window] [{"atMs":0,"command":"get_token_stats","args":{"range":"6h"}},{"atMs":0,"command":"get_project_breakdown","args":{"range":"6h"}},{"atMs":5000,"command":"get_token_stats","args":{"range":"6h"}},{"atMs":5000,"command":"get_project_breakdown","args":{"range":"6h"}},{"atMs":10000,"command":"get_token_stats","args":{"range":"6h"}},{"atMs":10000,"command":"get_project_breakdown","args":{"range":"6h"}},{"atMs":15000,"command":"get_project_breakdown","args":{"range":"6h"}}]
```

Both mounted commands run at 0, 5,000, and 10,000 ms: one complete fan-out per
fixed window and no interval below 5,000 ms despite continuous events. After
the token-stats subscriber leaves, the shared timer remains live and refreshes
only the project key at 15,000 ms. Companion assertions verify a fresh remount
issues zero requests, stale unmounted entries revalidate in the background,
changed ranges isolate keys, concurrent subscribers coalesce, rejected work
does not poison the cache, and Strict Mode cleanup releases listeners/timers.

### Range-scoped frontend query evidence

The same Node suite executes the production pure query planner for all four
displayed widget ranges. Every Code Insights request is the one permitted
two-period comparison; Trends issues one 14-day request per source and no
extra 7-day runtime request. Skills and the visible Projects readout stay at
the selected 6-hour range.

```text
[range-query-window] [{"view":"usage","hook":"useCodeInsights","displayedRange":"1h","command":"get_token_history","requestedRange":"2h","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"1h","command":"get_code_stats_history","requestedRange":"2h","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"1h","command":"get_llm_runtime_stats","requestedRange":"2h","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"6h","command":"get_token_history","requestedRange":"12h","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"6h","command":"get_code_stats_history","requestedRange":"12h","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"6h","command":"get_llm_runtime_stats","requestedRange":"12h","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"24h","command":"get_token_history","requestedRange":"2d","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"24h","command":"get_code_stats_history","requestedRange":"2d","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"24h","command":"get_llm_runtime_stats","requestedRange":"2d","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"7d","command":"get_token_history","requestedRange":"14d","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"7d","command":"get_code_stats_history","requestedRange":"14d","window":"comparison"},{"view":"usage","hook":"useCodeInsights","displayedRange":"7d","command":"get_llm_runtime_stats","requestedRange":"14d","window":"comparison"},{"view":"trends","hook":"useWeeklyTrends","displayedRange":"7d","command":"get_token_history","requestedRange":"14d","window":"comparison"},{"view":"trends","hook":"useWeeklyTrends","displayedRange":"7d","command":"get_code_stats_history","requestedRange":"14d","window":"comparison"},{"view":"trends","hook":"useWeeklyTrends","displayedRange":"7d","command":"get_llm_runtime_stats","requestedRange":"14d","window":"comparison"},{"view":"usage","hook":"useBreakdownData","displayedRange":"6h","command":"get_skill_breakdown","requestedRange":"6h","window":"current"},{"view":"usage","hook":"useBreakdownData","displayedRange":"6h","command":"get_project_breakdown","requestedRange":"6h","window":"current"}]
```

The assertion converts every logged range to milliseconds: a request beyond
the displayed window must be marked `comparison` and equal exactly `2 × R`;
all others must be at or below `R`. The mode-transition log records the lazy
secondary breakdown behavior:

```text
[breakdown-query-transition] [{"mode":"sessions","queries":[{"command":"get_session_breakdown","args":{"range":"6h","hostname":null,"limit":200}},{"command":"get_project_breakdown","args":{"range":"6h"}}]},{"mode":"projects","queries":[{"command":"get_project_breakdown","args":{"range":"6h"}}]},{"mode":"skills","queries":[{"command":"get_skill_breakdown","args":{"range":"6h","provider":null,"allTime":false,"limit":100}},{"command":"get_project_breakdown","args":{"range":"6h"}}]}]
```

Projects mode has one project command-and-args key, not a selected request plus
a hidden duplicate. Sessions and Skills retain one secondary project request
because the exact Projects readout remains visible; Skills explicitly sends
`allTime: false` with the selected range. Stable serialization still coalesces
equivalent argument order, while `6h` and internal `12h` remain isolated keys.
## Residual model-query follow-up attempt

The follow-up used the same worker-owned model-backfilled copy, pinned endpoint,
release profile, and app-cache-empty `Storage` protocol. SQLite was 3.45.0.
Before query changes, SHA-256 hashes captured the exact serialized 30d/90d
overview and history responses. A hash-only pass after the final design matched
all four byte lengths and hashes before the final timed pass.

The implemented design replaced the all-range representative-project rank with
one bounded `idx_model_observations_chain_time` candidate seek per active scoped
chain, then applied the complete cross-chain timestamp, ordinal, binary text,
and row-id order in Rust. History and activity derive explicit half-open raw
intervals for bucket-crossing hours. Active sources and any `raw_pruned=1`
authority keys are materialized once as keyed temporary tables, so each raw
branch performs a bounded observed-time seek without repeated durable-table
authority probes.

| Query | Window | Final cold | Output bytes | SHA-256 | Gate |
| --- | --- | ---: | ---: | --- | --- |
| `get_model_usage_overview` | 30d | 4,155.851 ms | 12,761 | `ec00856281427f5f619a49ef19ba9f4da599301c64a8a8f8b1f5c6657d4ef601` | fail |
| `get_model_history` | 30d | 270.428 ms | 5,151 | `72c50ada905adb8aa26a46e095a4e85a5404f73308b381436f9d1652a4d33efe` | pass |
| `get_model_usage_overview` | 90d | 3,794.482 ms | 17,131 | `c9e291097a69fb5d18ce0d06a7ef15225fd2cd7ae3d440ced3627f7b746a0ee5` | fail |
| `get_model_history` | 90d | 296.463 ms | 14,771 | `d20200a035a7f988459a4616daa4165ad26c88ab87cc2708a3f2ea2055c36933` | pass |

History therefore clears the 500 ms gate with exact parity, but overview does
not. The final overview is roughly five times faster than the 19.6–19.7 second
post-A/B result, yet remains 7.6–8.3 times over budget. This attempt is not an
acceptance pass and the residual overview release gate stays open.

## Residual model-query final retry

The final retry preserved the first attempt's history/activity interval and
temporary-authority work. Before changing project selection, a focused SQLite
3.45 diagnostic materialized the production-equivalent overview scope and
consumed every packed per-chain candidate. The 30d stage returned 3,501 chain
rows and 3,485 candidates (1,188,656 packed bytes) in 52.632 ms; the 90d stage
returned 4,392 candidates (1,495,109 packed bytes) in 61.635 ms. Both plans
range-seek `idx_model_observations_chain_time`, but report
`USE TEMP B-TREE FOR RIGHT PART OF ORDER BY`.

That measurement showed the project candidate stage was not the remaining
3.8-4.2 second cost. Inspection found completed overview still ranked every
in-range attributed raw turn to compute the latest contiguous provider run.
The final correction removed both residual SQL sorts: project selection now
uses prepared descending time/ordinal prefix seeks plus unsorted exact-prefix
tie reads, and running-now pages the observed-time index. Rust retains a whole
timestamp/provider prefix across pages, applies the exact ordinal, BINARY
record/source, and row-id suffix, and stops only when each represented provider
finds a different predecessor or exhausts the range. SQLite 3.45 plan tests
require bounded index searches with no raw observation scan or temporary
ordering for all three reads.

The parity pass ran once before timing and matched the first attempt's four
serialized byte lengths and hashes exactly. The final timed pass then ran once
against the same model-backfilled disposable copy, pinned to
`2026-08-02T19:27:43Z`; OS page cache remained uncontrolled.

| Query | Window | Final retry cold | Warm | Output bytes | SHA-256 | Gate |
| --- | --- | ---: | ---: | ---: | --- | --- |
| `get_model_usage_overview` | 30d | 889.941 ms | 1.144 ms | 12,761 | `ec00856281427f5f619a49ef19ba9f4da599301c64a8a8f8b1f5c6657d4ef601` | fail |
| `get_model_history` | 30d | 278.429 ms | 1.118 ms | 5,151 | `72c50ada905adb8aa26a46e095a4e85a5404f73308b381436f9d1652a4d33efe` | pass |
| `get_model_usage_overview` | 90d | 896.938 ms | 1.147 ms | 17,131 | `c9e291097a69fb5d18ce0d06a7ef15225fd2cd7ae3d440ced3627f7b746a0ee5` | fail |
| `get_model_history` | 90d | 280.014 ms | 1.098 ms | 14,771 | `d20200a035a7f988459a4616daa4165ad26c88ab87cc2708a3f2ea2055c36933` | pass |

The final correction cuts overview to about 0.9 seconds with exact parity, but
both acceptance windows remain roughly 1.8 times above the 500 ms ceiling.
Under the one-retry measurement discipline, no further design or timing
variant was attempted. The task remains failed and the dirty retry worktree is
preserved for diagnosis.

## Instrumented completed-path acceptance retry

An opt-in stage collector was added before another query variant. It records
overview stage boundaries only when the performance harness enables it, so the
normal application path does not read the clock. Direct profiling first showed
that the immutable corpus had pending model backfill metadata and zero model
rollup rows; its raw fallback took more than 41 seconds and was not the target
completed path. This proved a writable disposable copy was required.

An authorized SQLite online backup produced a byte-identical copy with
`quick_check=ok`. The unchanged model backfill processed 4,201,401 rows in 190
chunks, produced 9,734 hourly rows, completed with `raw_pruned=0`, and left no
consistency mismatch. The first completed-path profile disproved prepared
project-chain round trips as the residual cause:

| Stage | 30d before | 30d after | 90d before | 90d after |
| --- | ---: | ---: | ---: | ---: |
| Complete overview | 884.766 ms | 293.223 ms | 879.325 ms | 285.573 ms |
| Scoped materialization | 340.678 ms | 15.215 ms | 308.310 ms | 18.121 ms |
| Represented providers | 314.215 ms | 7.663 ms | 316.307 ms | 10.328 ms |
| Project candidates | 48.372 ms | 61.875 ms | 59.088 ms | 58.287 ms |
| Activity | 82.632 ms | 106.928 ms | 92.736 ms | 93.378 ms |
| Running-now | 29.159 ms | 33.697 ms | 28.898 ms | 30.971 ms |

Both dominant SQL branches bounded the outer observation window but selected
the raw rollup edges with `(ts < rollup_start OR ts >= rollup_end)`. SQLite
therefore walked the full in-window raw range before rejecting closed hours.
The correction splits each leading and trailing edge into its own half-open
`UNION ALL` branch and pins all four branches to
`idx_model_observations_observed_provider`.

Corpus query plans confirm two-sided observed-time index searches. The 30d
leading/trailing edges contain 160/47 rows and the 90d edges contain 23/47
rows. No branch scans model observations or creates a temporary ordering. The
post-change hybrid parity test also passes with `raw_pruned=1` authority.

The final one-shot measurements used the same completed copy and pinned
endpoint `2026-08-02T19:27:43Z`. Every cold query clears 500 ms and exactly
matches the established serialized response:

| Query | Window | Cold | Warm | Output bytes | SHA-256 | Gate |
| --- | --- | ---: | ---: | ---: | --- | --- |
| `get_model_usage_overview` | 30d | 297.203 ms | 1.065 ms | 12,761 | `ec00856281427f5f619a49ef19ba9f4da599301c64a8a8f8b1f5c6657d4ef601` | pass |
| `get_model_history` | 30d | 255.077 ms | 1.029 ms | 5,151 | `72c50ada905adb8aa26a46e095a4e85a5404f73308b381436f9d1652a4d33efe` | pass |
| `get_model_usage_overview` | 90d | 270.769 ms | 1.025 ms | 17,131 | `c9e291097a69fb5d18ce0d06a7ef15225fd2cd7ae3d440ced3627f7b746a0ee5` | pass |
| `get_model_history` | 90d | 288.437 ms | 1.047 ms | 14,771 | `d20200a035a7f988459a4616daa4165ad26c88ab87cc2708a3f2ea2055c36933` | pass |

The source corpus retained its canonical SHA-256, 13,525,123,072-byte size,
read-only mode, and unchanged WAL/SHM metadata. The disposable copy passed a
final `quick_check`, reported completed generation 190 and `raw_pruned=0`, had
no open handles, and was removed with its WAL, SHM, and temporary directory.
