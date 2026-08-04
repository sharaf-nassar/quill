# Contract: Model Analytics IPC

Four Tauri commands expose the current Models overview, session paging,
session detail, and retained-history retry surfaces. Serialized Rust fields use
camelCase and every command rejects with the shared structured error below.

## Shared types

```ts
type ModelRange = "1h" | "6h" | "24h" | "7d" | "30d";

type ModelIdentity = {
  provider: string;
  modelId: string;
};

type ModelBackfillStatus = {
  generation: number;
  trigger: "migration" | "startup_resume" | "retry" | "reconcile";
  status: "pending" | "running" | "complete" | "partial" | "failed";
  totalRoots: number;
  completedRoots: number;
  failedRoots: number;
  inventoryComplete: boolean;
  totalSources: number;
  processedSources: number;
  failedSources: number;
  skippedSources: number;
  remainingSources: number;
  observationsWritten: number;
  startedAt: string | null;
  updatedAt: string;
  finishedAt: string | null;
  lastError: string | null;
};
```

`provider: null` means all providers. Concrete providers use Quill's provider
vocabulary. Model IDs remain opaque strings but must pass the ingest boundary's
non-empty, scalar-count, and control-character validation.

The shipped TypeScript `ModelRange` and UI expose the five values above. Rust's
validator also accepts `90d` for direct internal callers.

## Command: `get_model_usage_overview`

### Request

```ts
invoke("get_model_usage_overview", {
  range: ModelRange,
  provider: string | null,
});
```

### Response

```ts
type ModelUsageOverviewResponse = {
  generatedAt: string;
  range: ModelRange;
  provider: string | null;
  representedProviders: string[];
  scope: {
    globalSessionCount: number;
    scopedSessionCount: number;
    scopedEvidenceCount: number;
    inventoryComplete: boolean;
    scopeFinal: boolean;
  };
  backfill: ModelBackfillStatus;
  buildingIndex?: boolean;
  totals: {
    sessions: number;
    projects: number;
    turns: number;
    attributedTokens: number;
    totalTokens: number;
    coveragePercent: number | null;
    distinctModels: number;
    multiModelSessions: number;
  };
  runningNow: Array<{
    provider: string;
    modelId: string;
    lastSeenAt: string;
    runningSinceAt: string;
    previousModelId: string | null;
  }>;
  models: Array<{
    identity: ModelIdentity;
    sessions: number;
    sessionPercent: number | null;
    projects: number;
    turns: number;
    primaryIn: number;
    daysActive: number;
    attributedTokens: number;
    sharePercent: number | null;
    firstSeen: string;
    lastSeen: string;
  }>;
  activity: {
    bucketSeconds: number;
    bucketStarts: string[];
    series: Array<{
      identity: ModelIdentity;
      sessionsPerBucket: number[];
    }>;
  };
  projectMatrix: Array<{
    project: string;
    totalSessions: number;
    cells: Array<{
      identity: ModelIdentity;
      sessions: number;
    }>;
  }>;
  combinations: {
    single: number;
    dual: number;
    threePlus: number;
    topPairs: Array<{
      a: ModelIdentity;
      b: ModelIdentity;
      sharedSessions: number;
    }>;
  };
  delegation: {
    parentTokens: number;
    subagentTokens: number;
    parentTop: {
      identity: ModelIdentity;
      sharePercent: number;
    } | null;
    subagentTop: {
      identity: ModelIdentity;
      sharePercent: number;
    } | null;
  };
};
```

The command returns the complete current Models-page payload from one SQLite
read snapshot. `models` is provider-qualified carry-forward attribution;
`activity` is a zero-filled fixed bucket axis; `projectMatrix`, `combinations`,
and `delegation` are bounded overview facets. `buildingIndex` is true while the
hourly model rollup is not complete and the reader therefore preserves its raw
evidence path.

`globalSessionCount` is independent of the selected scope.
`scopedSessionCount`, `scopedEvidenceCount`, represented providers, totals, and
facets require actual normalized observations inside the half-open selected
range and optional provider filter. Suppressed sources contribute nothing.
`scopeFinal` is true only when inventory is complete, status is `complete`, no
root or source failed, and no source remains.

## Command: `get_model_sessions`

### Request

```ts
invoke("get_model_sessions", {
  range: ModelRange,
  modelProvider: string,
  modelId: string,
  cursor?: string | null,
  limit?: number | null,
});
```

`limit` defaults to 20 and is clamped to 1 through 100. `cursor` is opaque and
belongs to the exact range and model identity that produced it.

### Response

```ts
type ModelSessionsResponse = {
  identity: ModelIdentity;
  total: number;
  nextCursor: string | null;
  sessions: Array<{
    provider: string;
    sessionId: string;
    displayName: string;
    cwd: string | null;
    hostname: string | null;
    selectedModelTokens: number;
    selectedModelTurns: number;
    lastActivityAt: string;
    primaryModel: ModelIdentity;
    distinctModels: number;
    hasWithinChainSwitches: boolean;
    chainCount: number;
  }>;
};
```

Sessions order by last activity descending, then provider and session ID
ascending. Every aggregate is limited to the requested range. A malformed,
stale, or foreign cursor returns `invalid_cursor` rather than an empty page.

## Command: `get_session_model_history`

### Request

```ts
invoke("get_session_model_history", {
  provider: string,
  sessionId: string,
  range: ModelRange,
});
```

### Response

```ts
type SessionModelHistoryResponse = {
  provider: string;
  sessionId: string;
  displayName: string;
  primaryModel: ModelIdentity | null;
  distinctModels: number;
  switchCount: number;
  attributedTokens: number;
  unattributedTokens: number;
  chains: Array<{
    chainId: string;
    parentChainId: string | null;
    kind: "parent" | "subagent";
    agentId: string | null;
    switchCount: number;
    attributedTokens: number;
    unattributedTokens: number;
    segments: Array<
      | {
          kind: "model";
          identity: ModelIdentity;
          startedAt: string;
          endedAt: string;
          turnCount: number;
          attributedTokens: number;
        }
      | {
          kind: "modelGap";
          startedAt: string;
          endedAt: string;
          turnCount: number;
        }
    >;
  }>;
};
```

Consecutive same-model turns compress into one model segment. Consecutive
null-model turns compress into a `modelGap`, which resets adjacency. Parent and
subagent chains remain separate. A session with no retained evidence in the
selected range returns `not_found`.

## Command: `retry_model_history_backfill`

### Request and response

```ts
const status = await invoke<ModelBackfillStatus>(
  "retry_model_history_backfill",
);
```

If no retained-history run is scheduled, retry advances the generation,
commits a `pending` status with trigger `retry`, emits that committed status,
and schedules the worker. If one is already scheduled, it returns the current
status without starting another run. Retry never deletes valid observations.

## Current refresh contract

The current Models view mounts `useModelAnalytics`, which calls only
`get_model_usage_overview` plus `retry_model_history_backfill`. Session paging
and session history remain supported IPC commands, but today's Models view has
no inspect panel and does not mount detail hooks.

Overview cache identity is the command plus exact `{ range, provider }`
arguments. Entries live for 45 seconds across React unmounts. A data-changing
`model-analytics-updated` event invalidates matching entries; status-only events
with `dataChanged: false` do not. Mounted visible subscribers join the widget's
shared fixed five-second refresh fan-out. Hidden subscribers stay stale and
refresh when visible again.

While the Models view is active, a 60-second fallback poll refreshes the same
overview identity. Hidden or inactive polls defer one refresh until the view is
observable. A same-scope refresh retains the last accepted response beside its
refresh or error state. Accepted overview responses merge backfill status by
generation and monotonic lifecycle/progress facts; a delayed older snapshot
cannot roll status backward. Overview Retry retries only the overview request;
backfill Retry uses its separate guarded command state.

## Event: `model-analytics-updated`

```ts
type ModelAnalyticsUpdatedEvent = {
  generation: number;
  status: "pending" | "running" | "complete" | "partial" | "failed";
  dataChanged: boolean;
  updatedAt: string;
};
```

Events are advisory and follow committed state or data changes. They never carry
overview rows; clients refetch authoritative command responses.

## Error contract

```ts
type ModelAnalyticsErrorCode =
  | "invalid_range"
  | "invalid_provider"
  | "invalid_model_id"
  | "invalid_cursor"
  | "not_found"
  | "storage_error";

type ModelAnalyticsError = {
  code: ModelAnalyticsErrorCode;
  message: string;
};
```

Every query and retry command rejects with this serialized object, never a plain
string. Validation messages are bounded and user-safe. Storage details remain
in local logs. Unexpected errors do not become empty successful responses.
