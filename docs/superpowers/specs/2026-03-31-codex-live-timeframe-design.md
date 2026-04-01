# Codex Live Timeframe Design

## Goal

Replace the low-signal Activity Pulse block in the Codex live pane with a compact timeframe selector that scopes the entire Codex live section to the selected recent window.

## Scope

This design only changes the Codex live module in the main window's live pane. Claude live usage rows, analytics tabs, storage schema, and unrelated session views are out of scope.

## User Outcome

Users can switch the Codex live section between `1h`, `6h`, `12h`, and `24h` from a control in the top-right of the section header. The summary counts, token total, session ordering, active session list, and freshness indicators all reflect that selected window. The Activity Pulse block is removed entirely.

## Current Constraints

The existing `useCodexLiveData` hook is hard-coded to a one-hour window and exposes `tokens1h`, `activityPulse`, `turnCount24h`, and per-session lifetime-ish code-change totals from `get_batch_session_code_stats()`. The backend token history API supports `1h`, `24h`, `7d`, and `30d`, while session breakdown is day-based and code-change stats are not time-windowed.

Those constraints mean `6h` and `12h` cannot be represented honestly by simply relabeling existing fields. The design must either derive the window from a broader raw data set or stop showing fields that cannot be truthfully range-scoped.

## Recommended Approach

Fetch a 24-hour Codex data set once per refresh and derive the selected `1h`, `6h`, `12h`, or `24h` window client-side inside the live hook. The UI should remove the Activity Pulse and also remove per-session row metadata that cannot be made truthful for arbitrary windows without deeper backend work.

This keeps the change isolated to the live module and its hook, avoids expanding Rust IPC for now, and makes the section simpler instead of adding a selector on top of mixed-scope data.

## UX Design

### Header

The Codex live header keeps the provider badge and "Session activity" label on the left. The right side becomes a compact controls cluster containing:

- a native `select` control with `1h`, `6h`, `12h`, and `24h`
- the existing freshness timestamp as secondary text

The selector should use the same restrained dark styling as the rest of the flattened module: subtle border, low-contrast background, clear focus ring, and no oversized pill treatment.

### Summary Rail

The summary rail stays as three compact metric blocks:

- Sessions
- Projects
- Tokens

Each metric's value and sparkline should be derived from the selected timeframe, and the tokens label should reflect the chosen range rather than staying pinned to `1H`.

### Session Ledger

The ledger continues to list the top active sessions for the selected range, ordered by tokens in that range and then recency. Each row keeps:

- project name
- project path or session/host fallback
- token total for the selected range
- relative last-active timestamp

The row-level `turns 24h` and `loc` metadata should be removed because the current backend does not provide truthful `6h` or `12h` values for those fields.

### Removed Element

The Activity Pulse block is removed with no replacement. The remaining summary spark lines already provide enough directional context without a second token chart.

## Data Design

### Input Window

The hook should fetch a 24-hour provider-scoped token history and a one-day provider-scoped session breakdown on each refresh. That provides enough raw material to derive `1h`, `6h`, `12h`, and `24h` windows without additional API surface.

### Derived Range State

Introduce a dedicated live range type for the Codex module with values `1h`, `6h`, `12h`, and `24h`. The selected range is owned by the component and passed into the live data hook.

### Derived Metrics

Inside the hook:

- filter token history to the selected cutoff
- filter sessions by `last_active >= cutoff`
- fetch per-session token history over 24 hours, then sum only the points within the selected cutoff
- compute summary counts from the filtered sessions
- compute active project count from the filtered sessions
- build summary sparklines from the filtered histories using bucket series sized for the selected range
- sort sessions by selected-range token total, then by last activity

Per-session code stats should not be fetched for this version because they are no longer displayed.

## Component Changes

### `src/components/live/CodexLiveModule.tsx`

- add local state for the selected live range
- render the header `select`
- update labels/tooltips to reference the selected range
- remove the Activity Pulse section
- simplify session rows to the range-truthful fields only

### `src/hooks/useCodexLiveData.ts`

- accept a live range argument
- fetch 24-hour token data and derive selected-window values client-side
- replace hard-coded one-hour field naming with range-neutral names
- remove `activityPulse` from the returned shape
- stop fetching batch session code stats

### `src/types.ts`

- add a live-range type for `1h | 6h | 12h | 24h`
- rename Codex live data fields so they are not semantically tied to `1h` or `24h`
- remove pulse data and row fields that are no longer rendered

### `src/styles/index.css`

- style the compact header control cluster
- preserve the flatter visual language introduced in the previous pass
- rebalance spacing after removing the pulse block

## Error Handling

The selector should never blank the module by itself. If a refresh fails and cached data exists, the module should continue to render the last good data for the selected range. If the initial fetch fails, the existing error state remains acceptable.

## Testing and Verification

Verification should rely on typecheck, lint, build, and a manual sanity pass of the live pane across the four range options. The most important correctness checks are:

- each range updates all visible live metrics together
- the session list reorders when the selected range changes
- the token label and tooltips match the selected range
- the module remains responsive at existing narrow breakpoints
- no stale Activity Pulse markup or styles remain

## Non-Goals

- Adding `6h` or `12h` support to analytics tabs
- Adding new Rust commands only to preserve row-level turn or LOC metrics
- Redesigning Claude's live provider section
