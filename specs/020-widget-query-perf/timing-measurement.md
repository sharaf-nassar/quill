# Widget query timing measurement

This acceptance record freezes the BEFORE query costs and corpus identity used
by feature 020's later A/B measurements.

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
which bypasses all app-level caches while retaining production SQL and Rust
post-processing.

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
