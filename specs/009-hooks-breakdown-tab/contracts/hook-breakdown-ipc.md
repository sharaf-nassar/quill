# Contract: `get_hook_breakdown` Tauri IPC

New Tauri IPC command exposed by `src-tauri/src/lib.rs` and consumed by
`src/hooks/useBreakdownData.ts`. Mirrors `get_skill_breakdown` in
parameters, response shape, and refresh semantics.

## 1. Command signature (Rust side)

```rust
#[tauri::command]
async fn get_hook_breakdown(
    days: i32,
    provider: Option<String>,
    all_time: bool,
    limit: Option<i64>,
) -> Result<Vec<HookBreakdown>, String> {
    let storage = STORAGE.get_or_init(/* … */);
    run_blocking(move || {
        storage.get_hook_breakdown(
            days,
            provider.as_deref().map(IntegrationProvider::from),
            all_time,
            limit,
        )
    })
    .await
}
```

Registered in the `invoke_handler` macro alongside `get_skill_breakdown`.

## 2. Parameters

| Field | Type | Notes |
|-------|------|-------|
| `days` | i32 | Active timeframe window in days. Maps the existing 1h/24h/7d/30d enum (1h is encoded as `0` in the Skills path, preserved here for consistency). Ignored when `all_time = true`. |
| `provider` | `Option<String>` | `"claude"`, `"codex"`, or `None` (= All). |
| `all_time` | bool | When true, return rows across all indexed history. Bypasses `days`. |
| `limit` | `Option<i64>` | Optional row cap. Matches the same cap used by `get_skill_breakdown`. |

## 3. Response

```rust
#[derive(Serialize)]
pub struct HookBreakdown {
    pub hook_identity: String,
    pub hook_event: String,
    pub tool_name: Option<String>,
    pub is_quill: bool,
    pub codex_count: i64,
    pub claude_count: i64,
    pub total_count: i64,
    pub last_fired_at: String, // ISO-8601 with offset
}
```

Wire encoding mirrors existing analytics IPC envelopes. Sort order:
`total_count DESC, hook_identity ASC`.

## 4. Frontend consumer

```ts
// src/hooks/useBreakdownData.ts

export type HookBreakdownRow = {
  hookIdentity: string;
  hookEvent: string;
  toolName: string | null;
  isQuill: boolean;
  codexCount: number;
  claudeCount: number;
  totalCount: number;
  lastFiredAt: string;
};

export function useHookBreakdown(args: {
  days: number;
  provider: "claude" | "codex" | null;
  allTime: boolean;
  limit?: number;
}): {
  data: HookBreakdownRow[];
  loading: boolean;
  error: string | null;
} {
  // follows the existing state pattern in useBreakdownData.ts:
  //   - useState data/loading/error
  //   - useRef firstLoadRef
  //   - useEffect fetch on mount + arg change
  //   - 60-second interval refresh
  //   - Tauri event listener on the same channel used by Skills refresh
}
```

Naming follows the existing TypeScript camelCase convention; the boundary
mapper (`fromHookBreakdownPayload`) converts the Rust snake_case fields
once at the IPC boundary.

## 5. Refresh wiring

The same Tauri event listeners that already refresh the Skills breakdown
trigger the Hooks breakdown:

| Event | Source | Reason |
|-------|--------|--------|
| `skill-usages-updated` | `Storage::store_skill_usages_for_messages` after a batch insert | Hook ingestion happens in the same indexing pass; refresh on the same boundary |
| `hooks-observed-updated` | NEW — emitted by `Storage::store_codex_hook_observation` after a successful insert | Codex live-fire refresh |

A new event channel `hooks-observed-updated` is introduced. The frontend
hook subscribes to both channels and debounces refreshes by 1 second,
matching the existing data-hook convention documented in
`lat.md/frontend#Custom Hooks#State Pattern`.

## 6. UI rendering rules (BreakdownPanel)

- Mode selector: existing tab strip adds a "Hooks" entry after "Skills".
- Empty state: same copy and styling as Skills empty state.
- Row layout:
  - Left: hook identity, including the `quill:` prefix when present. The
    `isQuill` field remains available for callers that need
    Quill-managed row classification.
  - Middle: total fires (provider-summed when filter strip is on All).
  - Right: relative `last fired` timestamp (matches Skills "Last used").
- Provider filter strip:
  - All → `total_count` displayed
  - Claude → `claude_count` displayed; rows with `claude_count == 0`
    are hidden
  - Codex → `codex_count` displayed; rows with `codex_count == 0` are
    hidden
- ALL TIME chip toggles `all_time` arg between false and true.
- Header help affordance (`?` button identical to InsightCard pattern)
  renders the FR-017 tooltip text:
  > "Claude hooks are tracked per script. Codex hooks are tracked per
  > event because Codex doesn't log per-script hook executions."

## 7. Error and edge handling

- Empty response → empty list, no row rendered, empty-state copy shown.
- IPC error → row list stays at last successful value, error indicator on
  the breakdown header (existing convention).
- A row whose `hook_identity` is too long for the UI is truncated visually
  with `text-overflow: ellipsis`; the full identity is shown on hover via
  the existing tooltip pattern in `BreakdownPanel.tsx`.
