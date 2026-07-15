# Contract: Model Analytics IPC

These Tauri commands return bounded summary, table, history, and backfill data for
the Models tab. Serialized Rust fields use camelCase.

## Shared types

```ts
type ModelRange = "1h" | "24h" | "7d" | "30d";

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

`provider: null` means all providers. A concrete provider uses Quill's existing
provider vocabulary; model IDs remain unconstrained opaque strings.

## Command: `get_model_analytics`

### Request

```ts
invoke("get_model_analytics", {
  range: ModelRange,
  provider: string | null,
});
```

### Response

```ts
type ModelAnalyticsResponse = {
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
  summary: {
    attributedTokens: number;
    unattributedTokens: number;
    totalTokens: number;
    attributedCoveragePercent: number | null;
    distinctModels: number;
    multiModelSessions: number;
  };
  models: ModelUsageRow[];
  backfill: ModelBackfillStatus;
};

type ModelUsageRow = {
  identity: ModelIdentity;
  attributedTokens: number;
  attributedSharePercent: number | null;
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens: number;
  cacheReadTokens: number;
  observedTurns: number;
  sessionCount: number;
  cacheReadSharePercent: number | null;
  firstSeen: string;
  lastSeen: string;
};
```

The backend returns every observed model in scope. Default order is attributed
tokens descending, then provider and raw model ID ascending. The frontend may
stably sort the complete response by any displayed column without changing
scope.

`globalSessionCount` counts distinct `(provider, analyticsSessionId)` pairs across
all unsuppressed retained source activity, independent of active range/provider.
`scopedSessionCount` counts distinct pairs with at least one normalized observation
timestamp inside the half-open active interval `[rangeStart, rangeEnd)`, joined to
unsuppressed source ownership and the provider filter. `scopedEvidenceCount`
counts accepted non-null model observations in that same scope.
`representedProviders` comes from any normalized observation timestamp inside the
selected interval joined to an unsuppressed source, so it can include a provider
with zero accepted model IDs. Source first/last interval overlap alone never proves
in-range activity.

Suppressed sources contribute no scope or empty-state facts.
`scope.inventoryComplete` equals persisted backfill completeness; it is not
inferred from status because a partial run can have complete root discovery plus
individual unreadable sources. `scopeFinal` is true only when status is complete,
inventory is complete, failed roots/sources are zero, and remaining sources are
zero. Only `scopeFinal` authorizes final empty claims.

### Empty-state precedence

1. `scopeFinal && globalSessionCount === 0`: no sessions exist.
2. `scopeFinal && scopedSessionCount === 0`: active provider/range has no session activity.
3. `scopeFinal && scopedSessionCount > 0 && scopedEvidenceCount === 0`: no reliable model
   evidence exists in scope.
4. Otherwise render recovered data plus provisional/incomplete scope wording,
   even when backfill is partial or failed.

## Command: `get_model_history`

### Request

```ts
invoke("get_model_history", {
  range: ModelRange,
  provider: string | null,
  selectedModel: ModelIdentity | null,
});
```

### Response

```ts
type ModelHistoryResponse = {
  generatedAt: string;
  range: ModelRange;
  provider: string | null;
  selectedModel: ModelIdentity | null;
  bucketSeconds: number;
  points: Array<{
    bucketStart: string;
    bucketEnd: string;
    attributedTokens: number;
    unattributedTokens: number;
    selectedModelTokens: number | null;
  }>;
};
```

Bucket widths are fixed by range: 300 seconds for `1h`, 3,600 for `24h`, 21,600
for `7d`, and 86,400 for `30d`. Empty buckets are returned as zeros so chart axes
remain stable. Selected-model tokens are a subset of attributed tokens and never
hide the aggregate attributed/unattributed series.

## Refresh contract

`ModelsTab` owns one `model-analytics-updated` listener and one 60-second fallback
poll while mounted. Backend events arrive after commit. The first unhandled event
opens a fixed one-second coalescing window; later events join that window without
extending its deadline. One trailing refresh starts when the window closes, so a
continuous event stream cannot starve the five-second visibility target.

Each event or poll refreshes aggregates and history and advances one
frontend-only monotonically increasing refresh generation consumed by mounted
detail hooks. Model selection alone refetches history and selected-model sessions,
not aggregates. Range or provider changes create a new request identity and clear
selection when that identity is no longer represented. Responses from an older
identity or generation are ignored.

Aggregates and history expose independent initial-loading, refresh-loading,
error, and retry state. Refreshing an unchanged request identity keeps its last
successful response visible until replacement succeeds. Each error notice retries
only its failed command for the current range, provider, and selected model. Data
from an old scope is never presented as belonging to a new scope.

Range and provider filters are separately labeled native-button groups using
`aria-pressed`, not tab roles. The history chart has an accessible label and a
visually hidden semantic table containing bucket bounds plus attributed,
unattributed, and selected-model values. Model inspection uses a native selection
button while the adjacent raw identifier remains selectable and copyable.

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
string. Validation messages are bounded and user-safe. Full storage/internal
details stay in local logs; the returned storage message remains generic.
Malformed or foreign keyset cursors use `invalid_cursor`; capped limits are not
errors; `not_found` is reserved for stale session detail. Unexpected errors are
not converted into empty successful responses.
