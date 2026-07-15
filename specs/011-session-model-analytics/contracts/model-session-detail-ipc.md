# Contract: Model Session Detail IPC

Detail commands page sessions for one raw model identity and lazily return
chain-separated model history for one selected session.

## Command: `get_model_sessions`

### Request

```ts
invoke("get_model_sessions", {
  range: "1h" | "24h" | "7d" | "30d",
  modelProvider: string,
  modelId: string,
  cursor: string | null,
  limit: number | null,
});
```

`modelId` passes the same raw-ID validation used at ingest. `limit` defaults to 20
and is capped at 100. `cursor` is opaque and must not be constructed by the
frontend.

### Response

```ts
type ModelSessionsResponse = {
  identity: {
    provider: string;
    modelId: string;
  };
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
    primaryModel: {
      provider: string;
      modelId: string;
    };
    distinctModels: number;
    hasWithinChainSwitches: boolean;
    chainCount: number;
  }>;
};
```

Sessions order by last activity descending, then provider and session ID
ascending. The cursor encodes the final row's stable order tuple. Every aggregate,
including primary model, is limited to the requested range. Selecting “Load more”
appends the next page until `nextCursor` is null.

## Command: `get_session_model_history`

### Request

```ts
invoke("get_session_model_history", {
  provider: string,
  sessionId: string,
  range: "1h" | "24h" | "7d" | "30d",
});
```

### Response

```ts
type SessionModelHistoryResponse = {
  provider: string;
  sessionId: string;
  displayName: string;
  primaryModel: {
    provider: string;
    modelId: string;
  } | null;
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
          identity: { provider: string; modelId: string };
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
null-model turns compress into a `modelGap`, which resets adjacency. Token-only
unattributed observations contribute to chain and session coverage totals but do
not create segments or reset model-turn adjacency.

Chains order parent first, then first activity, then chain ID. Segments order by
timestamp, source ordinal, and record key. Parent and subagent chains never merge.

## Refresh

Selected-model paging and session-history hooks consume the Models composition's
frontend-only refresh generation. When it advances, paging replays sequentially
from the first cursor through the number of pages currently loaded, deduplicates
by provider/session identity, and atomically replaces the prior page set. Existing
pages remain visible during replay and remain available with a page-local Retry if
replay fails.

On refresh-generation advance, selected-session page replay and currently
expanded session-history refetches start independently. Page replay failure never
suppresses expanded-history refresh, and row-history failure never prevents page
replacement. Collapsed cached histories invalidate and load again on their next
expansion. Successful results then reconcile stale rows; a refreshed `not_found`
removes its row after a bounded notice. Request-identity and generation guards
prevent old page or history responses from overwriting a newer scope.

## Loading and errors

Session pages load only after a model is selected. Chain history loads only after
a session row is expanded. Each level has independent loading, empty, and error
state so a detail failure does not replace the summary, table, or already loaded
pages.

A session removed between list and detail requests returns `not_found`; the UI
removes or refreshes that stale row after displaying a bounded notice.

Both commands reject with the shared `ModelAnalyticsError` envelope from
`model-analytics-ipc.md`. Malformed or foreign cursors use `invalid_cursor`;
missing stale session detail uses `not_found`.

## Interaction accessibility

Session rows use native disclosure buttons with stable panel IDs,
`aria-expanded`, and `aria-controls`. Loading panels expose `aria-busy` or a polite
status message. Load more and page/row Retry controls have scope-specific
accessible names.
