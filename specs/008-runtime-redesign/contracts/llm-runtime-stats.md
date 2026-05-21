# Contract — `get_llm_runtime_stats` IPC + Computation

**Feature**: 008 — Active-Time Runtime Tracking Redesign
**Owner**: `src-tauri/src/storage.rs` (computation) and
`src-tauri/src/lib.rs` (IPC binding)
**Consumer**: `src/hooks/useLlmRuntimeStats.ts` and
`src/components/analytics/NowTab.tsx`

This contract preserves the existing Tauri IPC shape so the React layer
is untouched. The computation behind it is rewritten to source from
`session_events`.

## IPC

### Request

```ts
invoke<LlmRuntimeStats>("get_llm_runtime_stats", {
    range: "1h" | "24h" | "7d" | "30d",
    scope?: "all" | "parent_only",
});
```

Unchanged from today. `scope` remains optional; absence and `"all"` are
equivalent.

### Response

```ts
interface LlmRuntimeStats {
    total_runtime_secs: number;   // sum of logical-turn durations
    turn_count:         number;   // count of logical turns in window
    session_count:      number;   // distinct (provider, session_id) chains touched
    avg_per_turn_secs:  number;   // total / turn_count, or 0 when empty
    sparkline:          number[]; // 7 buckets evenly dividing the range
}
```

Unchanged from today.

## Computation contract (server side)

### `Storage::get_llm_runtime_stats(range: &str, scope: Option<&str>) -> Result<LlmRuntimeStats, String>`

Constants:

```rust
const IDLE_THRESHOLD_SECS: f64 = 300.0;       // 5 minutes
const TOOL_WAIT_MAX_SECS:  f64 = 21_600.0;    // 6 hours
```

Obligations:

1. **STAT-1**: The lower-bound cutoff is computed in UTC via
   `(Utc::now() - range_to_duration(range)).to_rfc3339()` and compared
   against `session_events.timestamp` as a string with `?1` bound to
   that RFC3339 form (matches existing `response_times` pattern and
   satisfies FR-013).
2. **STAT-2**: Rows are selected with:
   ```sql
   SELECT timestamp, kind, provider, session_id, agent_id, is_sidechain
   FROM session_events
   WHERE timestamp >= ?1
     [AND is_sidechain = 0]  -- when scope = "parent_only"
   ORDER BY provider, session_id, COALESCE(agent_id, ''), timestamp;
   ```
   The bracket clause appears only on the `parent_only` branch.
3. **STAT-3**: The walker maintains:
   - a "current chain" key `(provider, session_id, agent_id)`,
   - a `turn_start_ms` (set when a new turn begins),
   - a `prev_ms` and `prev_kind` (set on every row),
   - aggregate accumulators (`total_secs`, `turn_count`, `sessions
     HashSet`, `bucket_sums[7]`).
4. **STAT-4**: On each row, the walker classifies the gap from
   `prev_ms`/`prev_kind` to `current_ms`/`current_kind`:
   - If the chain key changed, the previous turn is flushed; the
     current row starts a new turn (`turn_start_ms = current_ms`).
   - Else if the gap is a **tool-loop gap**
     (`prev_kind == asst_tool_use` AND `current_kind == user_tool_result`),
     the gap counts toward the active interval up to
     `TOOL_WAIT_MAX_SECS`. The current turn continues.
   - Else if the gap is `<= IDLE_THRESHOLD_SECS`, the current turn
     continues.
   - Else (gap exceeds idle threshold and is not a tool-loop gap), the
     previous turn flushes and the current row starts a new turn.
5. **STAT-5**: Flushing a turn:
   - `dur = last_event_ts - first_event_ts`, computed in seconds.
     `dur < 0` is clamped to `0` (defense against clock skew).
   - On `dur > 0`, increment `turn_count`, add to `total_secs`, and
     credit the duration to the sparkline bucket that contains the
     turn's start timestamp. Whole-turn assignment matches the prior
     `response_times`-based implementation and keeps
     `sum(sparkline) == total_runtime_secs` invariant; proration of a
     turn across multiple buckets is a future enhancement once we have
     multi-bucket coverage tests.
6. **STAT-6**: `session_count` is the cardinality of the
   `(provider, session_id)` set across all rows seen (not the chain
   tuple, so a parent + its sub-agents count as one session).
7. **STAT-7**: `avg_per_turn_secs = total_secs / turn_count`, or `0.0`
   when `turn_count == 0`.

## Frontend obligations

1. **UI-1**: No code change to `useLlmRuntimeStats.ts`. The hook
   continues to debounce-refresh on the `sessions-index-updated`
   Tauri event.
2. **UI-2**: `NowTab.tsx` updates the `InsightCard` `description` prop
   string to describe the new semantics:
   > "Total active time across CC and Codex sessions in this window —
   > model generation, reasoning, and tool execution counted together.
   > User-idle gaps over 5 minutes are excluded; tool waits are not."
3. **UI-3**: No change to the subtitle template
   (`"{sessions} sessions · {turns} turns · avg {avg}"`); the meaning
   of "turns" updates to "logical turns" per the new computation, and
   the help tooltip documents this.

## Tests required by this contract

- `Tool-wait gap longer than 5 minutes does not split a turn`
- `User-idle gap longer than 5 minutes splits a turn`
- `Negative gap from clock skew clamps to zero`
- `parent_only scope excludes is_sidechain = 1 rows`
- `Sparkline bucket sum equals total_runtime_secs`
- `Empty window returns total=0, turn_count=0, session_count=0, avg=0`
- `Sub-agent chain is counted as part of the same session_count`
