# Contract: Model History Backfill

This contract defines startup scheduling, progress, retry, reconciliation, and
failure behavior for retained transcript history.

## Migration and startup

Migration 28 creates model tables and inserts singleton state as `pending` with
trigger `migration`. App setup schedules the worker without blocking window
startup or other Analytics views.

If setup finds `running`, the prior process was interrupted. It changes the state
to `pending`, sets trigger `startup_resume`, preserves committed observations, and
schedules the same worker. A process-wide guard permits only one runner.

## Command: `retry_model_history_backfill`

### Request

```ts
invoke("retry_model_history_backfill");
```

### Response

Returns the `ModelBackfillStatus` shape from `model-analytics-ipc.md`.

- If no worker runs, increment generation, set `pending` with trigger `retry`,
  clear only run-level counters/error, schedule the worker, and return state.
- If a worker already runs, return current state and do not start another.
- Never delete valid observations merely because retry was requested.
- Rejection uses the shared `ModelAnalyticsError` envelope; scheduling/storage
  failure returns `storage_error`.

## Worker lifecycle

1. Acquire process guard and set `running`.
2. Persist total/completed/failed provider-root counts as each root enumeration
   resolves, while keeping `inventoryComplete` false during `running`.
3. Assign each discovered source its stable owning root key and current generation,
   then publish total count.
4. Process changed sources independently; update counters after committed batches.
5. Skip unchanged and unchanged-suppressed sources.
6. Prune unseen sources only by persisted root key for roots whose enumeration
   completed.
7. At terminal resolution, set `inventoryComplete` true only when every root
   completed and every discovered source was attempted; a failed root keeps it
   false. Then resolve status and release the guard.

The worker must yield between bounded batches so normal Tauri commands and
Analytics remain responsive.

## Terminal state rules

- `complete`: inventory is complete and no source failed.
- `partial`: some sources or provider roots failed, but at least one source was
  successfully retained, skipped, or processed. Existing results stay usable and
  receive an incomplete label.
- `failed`: discovery or storage prevented any useful progress for this run.
  Existing results, if any, stay usable with the failure label.

`failedRoots` reports enumeration failures separately because their undiscovered
source count is unknowable. `failedSources` and `remainingSources` quantify known
unread/unprocessed history. A terminal partial caused only by unreadable sources
can still have `inventoryComplete: true`; pending, running, interrupted, or
root-incomplete states cannot. `lastError` is bounded and safe for UI display;
detailed path/error context stays in local logs.

## Reconciliation guarantees

- Unchanged history produces no observation writes and no aggregate change.
- Changed history atomically replaces only the changed source.
- Failed changed history retains its prior successful rows and marks them stale.
- Removed history disappears only after complete discovery proves removal.
- Parent and subagent sources cannot erase each other even when they share an
  analytics/root session ID.
- Retry does not clear the session-search mtime map or rebuild unrelated indexes.

## Deletion and retention

Existing session, project, and host deletion paths remove matching observations.
When a retained source still exists, its last successful hash is marked
suppressed. Backfill skips unchanged suppressed content as worker progress, but
that source contributes no analytics scope, provider filter, model row, or empty
state. A changed source remains suppressed until its complete replacement commits
and atomically clears suppression. Physical source removal explicitly deletes its
observations before its source row after complete discovery.

There is no model-specific time-based expiry. Observations leave storage only
through explicit session/project/host deletion, source disappearance proven by
complete discovery, or atomic replacement of changed source content. The `30D`
control limits queries only.

## Event: `model-analytics-updated`

The backend emits this advisory window event only after committed status or data
changes.

```ts
type ModelAnalyticsUpdatedEvent = {
  generation: number;
  status: "pending" | "running" | "complete" | "partial" | "failed";
  dataChanged: boolean;
  updatedAt: string;
};
```

Events may represent a batch and do not carry aggregates. From the first pending
event, clients coalesce for one fixed second without extending the deadline, then
refetch authoritative IPC responses. No event is required for a wholly unchanged
reconciliation pass except terminal status change.
