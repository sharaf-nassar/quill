# Frontend

The React 19 frontend is a multi-window Tauri application with custom hooks for IPC data fetching, Recharts for visualization, and pure CSS for styling.

## Entry Point

[[src/main.tsx]] routes to window-specific components based on the `?view=` URL parameter.

Each window gets its own Suspense boundary with a fallback. Per-window zoom persistence is stored in localStorage (`quill-zoom-{view}`) and supports Ctrl+/-, Ctrl+0 with a 0.5-2.0x range. Ctrl+F is blocked to prevent the webview's native find-in-page (no search UI exists). A `ToastProvider` context wraps all views for notifications, and [[src/hooks/useIntegrations.ts]] gates secondary windows when no provider is enabled.

### Window Routes

Seven Tauri windows are routed by the `?view=` URL parameter, each with its own Suspense boundary.

| Route | Component | Purpose |
|-------|-----------|---------|
| `?view=main` (default) | [[src/App.tsx]] | Split-pane live + analytics |
| `?view=sessions` | `SessionsWindowView` | Full-text session search |
| `?view=learning` | `LearningWindow` | Rules and memory optimization |
| `?view=plugins` | `PluginsWindowView` | Plugin management |
| `?view=restart` | `RestartWindowView` | Claude Code instance restart |
| `?view=runs` | `RunsWindowView` | Learning run history details |
| `?view=integrations` | `IntegrationsWindowView` | Anchored provider enable/disable popup |

## Main Window Layout

[[src/App.tsx]] implements a split-pane layout with a draggable divider separating the [[features#Live Usage View]] and [[features#Analytics Dashboard]].

The split ratio (0.15-0.85) persists in localStorage. The layout supports keyboard-driven resizing, pointer-anchored divider dragging, and window resize events. Usage data refreshes every 3 minutes via `fetch_usage_data()` only while a provider is active. Right-click shows a context menu with Refresh and Quit options, and the main panel swaps to an integration empty state with a provider rescan action when Claude Code and Codex are both inactive.

### Component Tree

The main window nests: `TitleBar` (feature buttons on the left, centered QUILL popup trigger, update/version/close on the right) at the top, `UsageDisplay` (live rate limit buckets) on the left, and `AnalyticsView` (tabbed analytics) on the right.

## Components

Components are organized by feature domain under `src/components/`.

### Core Components

Top-level UI chrome and live rate limit display shared across the main window.

- **TitleBar** (`src/components/TitleBar.tsx`) — Custom window chrome with left-aligned feature buttons, a QUILL trigger that opens the anchored integrations popup window, and version/close controls on the right.
- **ProviderMenu** (`src/components/integrations/ProviderMenu.tsx`) — Reusable provider action panel that shows Claude Code and Codex availability, enabled state, and the next enable or disable action.
- **ConfirmDialog** (`src/components/ConfirmDialog.tsx`) — Shared confirmation modal used for destructive provider cleanup and provider installation confirmation.
- **IntegrationsWindowView** (`src/windows/IntegrationsWindow.tsx`) — Popup-window host for `ProviderMenu` that auto-closes on blur and owns the confirmation-driven enable/disable flow.
- **UsageDisplay** (`src/components/UsageDisplay.tsx`) — Composes live sections from enabled providers, keeps the time mode selector and provider-error handling, and delegates provider-specific rendering to [[src/components/live/ClaudeLiveModule.tsx]] and [[src/components/live/CodexLiveModule.tsx]].
- **ClaudeLiveModule** (`src/components/live/ClaudeLiveModule.tsx`) — Claude-specific live bucket section that preserves the existing quota UI for API-backed limits.
- **CodexLiveModule** (`src/components/live/CodexLiveModule.tsx`) — Codex-specific workload module with three sparkline-backed summary stats, a ranked active-session ledger, and a header selector for 1h/6h/12h/24h live ranges with container-query reflow for narrow split-pane widths.
- **UsageRow** (`src/components/UsageRow.tsx`, 222 lines) — Individual rate limit visualization with three display modes: pace marker (vertical line), dual bars (time elapsed vs utilization), or background fill.

### Analytics Components

Recharts-based analytics in `src/components/analytics/` with three tabs: Now, Trends, and Charts.

- **NowTab** (214 lines) — Real-time metrics with range selector (1h/24h/7d/30d), six insight cards, a 24-hour activity heatmap, and a switchable breakdown panel (hosts/projects/sessions).
- Selecting a session in `NowTab` now keeps provider identity alongside `session_id`, so token charts, compact token stats, and delete actions stay scoped to the correct Claude or Codex session.
- **TrendsTab** (105 lines) — Token trends, code velocity, and cache efficiency charts with week-over-week comparison.
- **ChartsTab** (454 lines) — Composite Recharts chart with three axes (utilization, tokens, LOC). Lazy-loaded with Suspense.
- **UsageChart** (456 lines) — `ComposedChart` with Area, Line, and custom Tooltip. Uses `ChartCrosshairContext` for tooltip synchronization.
- **BreakdownPanel** (504 lines) — Sortable table showing hosts, projects, or sessions with token counts and turn counts. Session rows display provider badges and use provider-safe composite keys for selection.
- **Insight cards**: `InsightCard` (generic), `SessionHealthCard`, `ProjectFocusCard`, `LearningProgressCard` — each shows a metric with trend arrow and sparkline.
- **Sparklines**: `TokenSparkline`, `CodeSparkline`, `MiniChart` — small inline Recharts charts.
- **Utility**: `TabBar`, `TogglePills` (range selector), `ActivityHeatmap`, `CompactStatsRow`, `shared.tsx` (getColor, TrendArrow).

### Learning Components

Rule management and memory optimization UI in `src/components/learning/`.

- **MemoriesPanel** (807 lines) — Memory optimization UI with project selector, file browser with content preview, suggestion approval/denial, and custom project management. The largest frontend component.
- **RuleCard** — Displays a learned rule with name, confidence %, and a metadata row (domain, source, project) in muted text. For active rules (on disk): no state badge, delete only. For discovered rules (DB-only): state badge, promote button with inline two-step confirmation, and expandable DB-stored content preview.
- **SuggestionCard** (258 lines) — Memory optimization suggestion with approve/deny/undo actions and diff summaries.
- **StatusStrip** (79 lines) — Observation count, unanalyzed count, last run time, and "Run Analysis" button.
- **DomainBreakdown** (38 lines) — Rules-by-domain pie chart.
- **RunHistory** (204 lines) — Run list with status badges and per-phase breakdown.
- **FloatingRunsWindow** (104 lines) — Collapsible sidebar for run history.

### Session Components

Full-text session search UI in `src/components/sessions/` for a shared Claude-plus-Codex index.

- **SearchBar** (42 lines) — Query input with real-time validation.
- **FilterBar** — Multi-select filters for provider, project, host, role, date range, and git branch.
- **ResultCard** — Search hit preview with provider badge, snippet, and per-session code-change pill.
- **DetailPanel** — Context message display with provider badge, match highlighting, and session-local code-change totals.

### Plugin Components

Plugin management UI in `src/components/plugins/` with four tabs.

- **InstalledTab** — Plugin list with enable/disable controls.
- **BrowseTab** — Available plugins from connected marketplaces.
- **MarketplacesTab** — Add, remove, refresh marketplace repos.
- **UpdatesTab** — Available updates with bulk update support.

### Restart Component

Controls for restarting Claude Code instances from the dedicated Restart window.

- **RestartPanel** (`src/components/restart/RestartPanel.tsx`, 205 lines) — Instance list with status indicators, force restart option, and hook installation prompt.

## Custom Hooks

All data hooks use Tauri `invoke()` for request-response and `listen()` for push event refresh. Most refresh on a 60-second interval and debounce event-triggered refreshes by 1 second.

### Integration Hook

`useIntegrations` in [[src/hooks/useIntegrations.ts]] loads provider statuses, listens for `integrations-updated`, tracks per-provider in-flight actions, and drives the integrations popup window plus blocked-window gating.

### Data Fetching Hooks

Hooks that invoke Tauri commands and return async state (data, loading, error).

| Hook | Returns | Tauri Commands |
|------|---------|----------------|
| `useAnalyticsData` | Usage history, stats, snapshot count | `get_usage_history`, `get_usage_stats`, `get_snapshot_count` |
| `useTokenData` | Token history with hostname/session filtering | `get_token_history`, `get_token_stats`, `get_token_hostnames` |
| `useCodexLiveData` | Codex live-pane summary, range-aware token sparklines, active sessions/projects | `get_session_breakdown` (provider-scoped, 24h fetch), `get_token_history` (24h provider + per-session) |
| `useCodeStats` | Lines added/removed by language | `get_code_stats`, `get_code_stats_history` |
| `useBreakdownData` | Host/project/session breakdown tables | `get_host_breakdown`, `get_project_breakdown`, `get_session_breakdown` |
| `useSessionHealth` | Avg duration, tokens, sessions/day with trend | `get_session_stats` |
| `useActivityPattern` | 24-hour hourly token distribution | `get_token_history` (derived) |
| `useResponseTimeStats` | Avg/peak response time, idle time, sparkline | `get_response_time_stats` |
| `useEfficiencyStats` | Tokens-per-LOC ratio with trend | Derived from token + code stats |
| `useVelocityStats` | LOC-per-hour with trend | Derived from code stats |
| `useLearningStats` | Rule counts by state, confidence buckets | `get_learned_rules` (derived) |
| `useLearningData` | Rules, runs, settings, observations, logs | Multiple learning commands + events |
| `useMemoryData` | Memory files, suggestions, projects | Multiple memory optimizer commands |
| `useSessionCodeStats` | Batch LOC stats per session (ref-cached) | `get_batch_session_code_stats` |
| `usePluginData` | Installed plugins, marketplaces, updates | Multiple plugin commands |
| `useCacheEfficiency` | Cache hit rate (derived from token history) | None (derived) |

`useCodexLiveData` accepts a Codex-only live range (`1h`, `6h`, `12h`, `24h`) and derives the visible live metrics from a provider-scoped 24-hour token/session snapshot. The Codex live header pairs that selector with responsive container-query styling so the control cluster stays compact in narrow panes, while the module itself applies a ResizeObserver-driven compact mode keyed off the enclosing split pane when the live area gets short enough to collapse the summary rail above the ledger.

### State Pattern

Hooks follow a consistent async state pattern: `useState` for data/loading/error, `useRef` for initial load tracking, `useEffect` for fetching, periodic interval refresh, and Tauri event listener cleanup.

### Context

React Context providers used across the frontend for shared state.

- **ToastProvider** (`src/hooks/useToast.tsx`) — Notification system via React Context. Provides `toast(level, message)` to any component.
- **ChartCrosshairContext** (`src/components/analytics/ChartCrosshairContext.tsx`) — Synchronizes crosshair position across multiple Recharts charts.

## Type Definitions

[[src/types.ts]] contains all TypeScript types (434 lines), mirroring the Rust models in [[src-tauri/src/models.rs]].

Key type categories: usage/token tracking (`UsageBucket`, `TokenDataPoint`, `TokenStats`), analytics (`BucketStats`, `SessionHealthStats`, `ResponseTimeStats`), learning (`LearnedRule`, `LearningRun`, `LearningSettings`), session search (`SearchHit`, `SearchResults`, `SessionContext`), plugins (`InstalledPlugin`, `Marketplace`, `PluginUpdate`), restart (`ClaudeInstance`, `RestartStatus`), memory (`MemoryFile`, `OptimizationSuggestion`).

Display enums: `TimeMode`, `RangeType`, `TrendType`, `BreakdownMode`, `SortMode`, `AnalyticsTab`, `PluginsTab`.

## Styling

Pure CSS with no framework. Dark theme (`#121216` background, `#d4d4d4` text, 11px system sans-serif).

### Stylesheets

Per-window CSS files under `src/styles/`, each scoped to a specific feature domain.

| File | Lines | Scope |
|------|-------|-------|
| `src/styles/index.css` | 2,026 | Global styles, main window, analytics |
| `src/styles/learning.css` | 810 | Learning window and components |
| `src/styles/sessions.css` | 475 | Session search window |
| `src/styles/plugins.css` | 786 | Plugin manager |
| `src/styles/restart.css` | 294 | Restart window |

### Color System

Semantic color palette used across all stylesheets for consistent status indication.

- Green `#34d399`: success, utilization < 50%
- Yellow `#fbbf24`: warning, utilization 50-80%
- Red `#f87171`: error, utilization >= 80%
- Blue `#60a5fa`: accents, interactive elements
- Memory type badges: blue (user), red (feedback), green (project), yellow (reference), purple (claude-md)

### Responsive Scaling

The CSS variable `--s` scales all dimensions based on container size. Per-layout window sizes persist in localStorage (`quill-size-{live|analytics|both}`).

[[src/App.tsx]] now measures the rendered live content height at `--s: 1` before choosing the next scale, so provider modules shrink against the available split-pane height instead of relying on a Claude-sized fixed baseline.

The Codex live module adds container queries on top of `--s`, keeping its summary rail flat on wider panes, capping the active-session ledger width so extra room feeds the summary column, and reflowing those metrics into compact multi-column and stacked layouts before the session ledger loses readability. In the default two-column state, the summary rail stretches to the ledger height and divides that space across three equal stat rows. When the live pane gets short, compact-height mode flips that rail into a horizontal summary row above the ledger so the bottom sparkline does not clip. The live split pane top-aligns its content, allows a lower minimum `--s`, caps split-mode upscaling at `1`, and uses a module-level compact-height mode keyed to the outer live pane, including tighter spacing and fewer visible ledger rows, instead of allowing an internal scrollbar in the live section.

## Utilities

Shared formatting and chart helper functions under `src/utils/`.

| File | Exports |
|------|---------|
| `src/utils/format.ts` | `formatNumber()` (thousand separators), `formatDurationSecs()` (human-readable) |
| `src/utils/tokens.ts` | `formatTokenCount()` (1.2M, 5.4k display) |
| `src/utils/time.ts` | `timeAgo()` (ISO string to relative "5m ago") |
| `src/utils/chartHelpers.ts` | `formatTime()`, `dedupeTickLabels()`, `anchorToNow()`, `getAreaColor()` |
