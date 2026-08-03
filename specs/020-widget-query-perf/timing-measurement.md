# Widget query timing measurement

This acceptance record freezes the BEFORE query costs and corpus identity, then
records the interim post-A/B gate used to size slices C/D/E.

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
