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

The Models cold call remains expensive because
`get_model_usage_overview` still materializes its `scoped_overview` temporary
table. This MVP intentionally does not reduce that cold cost; its cache makes
warm repeats fast. The cold miss requires a follow-up that reopens the
temp-table-to-CTE question in Open Q7.
