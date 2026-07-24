# Analytics timing measurement

This non-gating measurement records the real production-size database pass
required by Q4 and the resulting follow-up decision.

## Environment and method

The measurement used `/home/mamba/.local/share/com.quilltoolkit.app/usage.db`
at 7,510,933,504 bytes (7.51 GB). A one-off read-only storage harness called
the same backend methods the IPC timing wrapper surrounds. It measured the
30-day all-provider Models overview twice in one process, then the 30-day Now
Context request twice with its normal limit of 1,000 rows.

The first call is the cold result and the immediate repeat is the warm result.
The harness was removed after the measurement; this document is the retained
record.

## Results

| Switch | Cold | Warm | Guidance | Result |
| --- | ---: | ---: | --- | --- |
| Models, 30d, all providers | 16,068 ms | 1 ms | ~1,500 ms cold / ~300 ms warm | Warm passes; cold misses |
| Now Context, 30d | 242 ms | 238 ms | ~1,500 ms cold / ~300 ms warm | Passes |

## Temp-table versus CTE follow-up

The follow-up used the same read-only production database at 7,512,981,504
bytes (7.51 GB), with a 30-day all-provider range ending at
2026-07-24T05:49:49Z. It replayed the nine scoped-set aggregate shapes that
drive the overview: session and token totals, session/model reach, model
metadata, bucket activity, delegation, and running-model metadata.

The control created `scoped_overview` once, created its two existing temp
indexes, and then ran those shapes. The alternative prefixed every shape with
the same `WITH scoped_overview AS MATERIALIZED (...)` definition. Both paths
returned identical row counts and values for all nine result sets.

| Workload | Cold | Warm endpoint | Decision |
| --- | ---: | ---: | --- |
| Existing temp table + indexes | 5,803 ms | 1 ms | Control |
| Per-statement materialized CTE | 16,838 ms | 1 ms | Reject |

`get_model_usage_overview` needs several separately consumed result sets.
SQLite scopes a CTE to one statement, so the direct CTE replacement rescans and
rematerializes the source rows for every statement. The indexed temp table
shares that work once. A single-statement JSON/result-set redesign could change
this tradeoff, but it would be a correctness-sensitive endpoint rewrite, not a
replacement of the current materialization.

The 1 ms warm figure is the actual same-process cache-hit timing recorded above;
both alternatives have identical cache behavior because the cache lookup occurs
before the uncached implementation. The CTE candidate is rejected. The existing
cache and temp-table implementation remain unchanged, while a later
optimization may pursue a deliberately designed single-statement response if the
cold guidance remains important.
