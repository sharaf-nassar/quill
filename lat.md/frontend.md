# Frontend

The React 19 frontend is a multi-window Tauri application with custom hooks for IPC data fetching, Recharts for visualization, and pure CSS for styling.

## Entry Point

[[src/main.tsx]] routes to window-specific components based on the `?view=` URL parameter.

Each window gets its own Suspense boundary with a fallback. Per-window zoom persistence is stored in localStorage (`quill-zoom-{view}`) and supports Ctrl+/-, Ctrl+0 with a 0.5-2.0x range via Tauri's native webview zoom API, falling back to CSS `zoom` only outside Tauri. Ctrl+F is blocked to prevent the webview's native find-in-page (no search UI exists). A `ToastProvider` context wraps all views for notifications, [[src/hooks/useIntegrations.ts]] gates provider-dependent secondary windows when no provider is enabled, and [[src/windows/SessionsWindowView.tsx]] refreshes the session index on demand before loading search facets.

### Window Routes

Eight Tauri windows are routed by the `?view=` URL parameter, each with its own Suspense boundary.

| Route | Component | Purpose |
|-------|-----------|---------|
| `?view=main` (default) | [[src/App.tsx]] | Split-pane live + analytics |
| `?view=sessions` | `SessionsWindowView` | Full-text session search |
| `?view=learning` | `LearningWindow` | Rules and memory optimization |
| `?view=plugins` | `PluginsWindowView` | Plugin management |
| `?view=restart` | `RestartWindowView` | Claude Code instance restart |
| `?view=runs` | `RunsWindowView` | Learning run history details |
| `?view=settings` | `SettingsWindowView` | Comprehensive feature toggle and runtime configuration surface |
| `?view=release-notes` | `ReleaseNotesWindow` | Browse published GitHub release notes |

The `settings` and `release-notes` routes are not gated on having an enabled provider; both stay reachable so users can manage integrations and browse changelog history regardless of integration state.

## Main Window Layout

[[src/App.tsx]] implements a split-pane layout with a draggable divider separating the [[features#Live Usage View]] and [[features#Analytics Dashboard]].

The layout supports two orientations controlled by a `LayoutMode` toggle (`"stacked"` or `"side-by-side"`) persisted in localStorage as `quill-layout-mode`. Stacked mode (default) places Live above Analytics with a horizontal divider; side-by-side mode places Live on the left and Analytics on the right with a vertical divider. Each orientation has an independent split ratio (0.15-0.85) persisted separately (`quill-split-ratio` for stacked, `quill-split-ratio-h` for side-by-side). The layout supports keyboard-driven resizing (ArrowUp/Down for stacked, ArrowLeft/Right for side-by-side), pointer-anchored divider dragging, and window resize events. Usage data refreshes every 3 minutes via `fetch_usage_data()` only while a provider is active. Right-click shows a context menu with Refresh and Quit options, and the main panel swaps to an integration empty state with a provider rescan action when Claude Code and Codex are both inactive.

The split panes keep their root content views shrinkable with `min-width: 0` on `UsageDisplay` and `AnalyticsView`, preventing intrinsic child widths from forcing the macOS window wider when switching orientations.

The shared `.content` class centers its children for full-pane loading and empty states, so each pane wrapper (`.live-content`, `.analytics-content`) overrides `align-items` to `stretch` and `justify-content` to `flex-start`. Without this override the pane re-centers vertically whenever its child changes height (for example when switching the Context tab range), which the user perceives as the page jumping to the middle even though `scrollTop` is unchanged.

### Component Tree

The main window nests `TitleBar` at the top, `UsageDisplay` and `AnalyticsView` in the panels area.

`TitleBar` has feature buttons on the left, a centered static `QUILL` brand label, and a right-side cluster with a cogwheel button (immediately left of the version) that opens the standalone [[features#Settings Window]], followed by the version and close controls. `UsageDisplay` shows live rate limit buckets. `AnalyticsView` renders tabbed analytics. In stacked mode, Live is above Analytics; in side-by-side mode, Live is on the left and Analytics on the right.

## Components

Components are organized by feature domain under `src/components/`.

### Core Components

Top-level UI chrome and live rate limit display shared across the main window.

- **TitleBar** (`src/components/TitleBar.tsx`) — Custom window chrome with left-aligned feature buttons, a centered static `QUILL` brand label, and a right-aligned cluster containing a cogwheel settings trigger (immediately left of the version) that opens an inline `ProviderMenu` popover with backdrop-based click-outside dismissal, followed by version and close controls. The popover uses the `provider-menu--right` modifier so it right-anchors to the cogwheel inside the narrow main window. When the frontend's periodic updater check finds a release, it also shows an `Update x.y.z` action that installs via [[src-tauri/src/lib.rs#install_app_update]] so the backend owns the restart handoff. The version label is rendered as a button that opens the `release-notes` window via [[src/windows/ReleaseNotesWindow.tsx]]. Owns the confirmation-driven enable/disable flow via `ConfirmDialog`.
- **ReleaseNotesWindow** (`src/windows/ReleaseNotesWindow.tsx`) — Standalone window that fetches published GitHub releases through the [[src-tauri/src/lib.rs#get_release_notes]] command, shows the latest first, and places Previous/Next navigation plus the selectable release URL in a top toolbar below the titlebar. Centers the release tag between the release counter and publish date, renders release bodies as sanitized GitHub-flavored Markdown that fills the scroll area, surfaces loading, empty, and error states with a Retry control, and supports Escape plus Left/Right arrow keyboard navigation.
- **ProviderMenu** (`src/components/integrations/ProviderMenu.tsx`) — Reusable provider action panel rendered as a compact terminal-utility list of 22 px rows separated by 1 px hairlines. Inline rows for Layout (stacked/side-by-side icon toggle), Status (compact `<select>` for the indicator primary provider), and Context (working-context preservation toggle) come first, followed by the `Integrations` group (Claude Code, Codex, MiniMax) and then `Brevity` (Claude Code, Codex). The Integrations group leads with a "Rescan PATH" row whose `RUN`/`...` toggle calls the `rescan` callback (which invokes the `rescan_integrations` IPC) so users can re-derive the login-shell PATH after installing a CLI or editing shell config without restarting. Each provider toggle is a single 36 px-min `pmenu-toggle` pill that resolves to one of `ON` / `OFF` / `N/A` / `SETUP` / `…` / `—` depending on `inFlightProviders`, `setupState`, `detectedCli`, and per-provider `enabled` flags, with semantic colors drawn from [[lat.md/frontend#Frontend#Styling#Color System]] (green = on, dim = off, red = unavailable, yellow = needs setup, blue = busy). Hovering any row in a section instantly shows a detailed `pmenu-tooltip` (one of `layout`, `status`, `context`, `brevity`, `integrations`) rendered via `react-dom/createPortal` into `document.body` to escape the popover's `overflow-y: auto`; the tooltip is positioned `fixed` to the left of the menu by default and falls back below the popover when there is no horizontal room for the 252 px panel, with a small CSS-rotated diamond pointing back at the source row. Tooltip copy lines support inline `<code>` rendering via a backtick parser. When a provider row shows N/A and `lastDetectionAttempts` is non-empty, hover replaces the generic Integrations tooltip with a per-row diagnostic listing every path Quill checked while looking for that provider's CLI. The portal layer dismisses on `mouseleave`, window resize, or menu scroll. Layout props remain optional for backward compatibility with the legacy `IntegrationsWindowView`.
- **ConfirmDialog** (`src/components/ConfirmDialog.tsx`) — Shared confirmation modal used for destructive provider cleanup and provider installation confirmation.
- **IntegrationsWindowView** (`src/windows/IntegrationsWindow.tsx`) — Legacy standalone window host for `ProviderMenu` (unused since inline popover migration).
- **UsageDisplay** (`src/components/UsageDisplay.tsx`) — Composes the shared workload summary rail, grouped provider limit sections, the detailed-row time mode selector, and provider-error handling for the main window's live pane.
- **LiveSummaryModule** (`src/components/live/LiveSummaryModule.tsx`) — Shared top-of-pane workload module with the 1h/6h/12h/24h selector, freshness label, and aggregate `Sessions`, `Projects`, and `Tokens` cards across the enabled providers.
- **ProviderUsageModule** (`src/components/live/ProviderUsageModule.tsx`) — Reusable provider section that renders quota rows with a provider badge and source note. For MiniMax, filters buckets to primary models (M\*, coding-plan-search, coding-plan-vlm) and shows an "All models" hover badge with a tooltip displaying the remaining models' name, utilization, and reset countdown.
- **UsageRow** (`src/components/UsageRow.tsx`, 222 lines) — Individual rate limit visualization with three display modes: pace marker (vertical line), dual bars (time elapsed vs utilization), or background fill. Exports `formatCountdown` and `gradientColor` utilities for reuse by tooltip renderers.

### Analytics Components

Recharts-based analytics in `src/components/analytics/` with Now, Trends, Charts, and a conditional Context tab.

- **NowTab** (214 lines) — Real-time metrics with range selector (1h/24h/7d/30d), six insight cards, a 24-hour activity heatmap, and a switchable breakdown panel (sessions/projects/hosts/skills).
- `NowTab` shares one comparison-range code-history fetch between the efficiency and velocity cards via `src/hooks/useCodeInsights.ts`, which avoids firing the same `get_code_stats_history` IPC call twice per refresh.
- Selecting a session in `NowTab` now keeps provider identity alongside `session_id`, so token charts, compact token stats, and delete actions stay scoped to the correct Claude or Codex session.
- **TrendsTab** (105 lines) — Token trends, code velocity, and cache efficiency charts with week-over-week comparison.
- **ChartsTab** (454 lines) — Composite Recharts chart with three axes (utilization, tokens, LOC). Lazy-loaded with Suspense.
- **ContextSavingsTab** — Context preservation analytics with a four-column stats strip (saved, indexed, returned, routing) over a stacked trend chart, breakdown table, and recent events feed. Breakdown rows render a relative-magnitude bar fill behind each row scaled to the largest event count, and recent events use a single-line log format with category swatches and a directional byte arrow (→ indexed, ← returned). Confidence is hidden for exact estimates. `AnalyticsView` shows this tab only when context preservation is enabled or historical context-savings events exist.
- **UsageChart** (456 lines) — `ComposedChart` with Area, Line, and custom Tooltip. Uses `ChartCrosshairContext` for tooltip synchronization.
- **BreakdownPanel** — Sortable table showing sessions, projects, hosts, or skills with compact count columns. It renders all rows in a flexing scroll area that fills the available analytics pane height instead of paginating the breakdown. Session rows display provider badges and use provider-safe composite keys for selection. Hosts and projects show `<recency>` in their time column (e.g. `2h ago`); sessions show `<recency> · <duration>` (e.g. `23h ago · 23h 43m`, or `active · 6m` when `last_active` is within the last 5 minutes), so the SQL `last_active DESC` ordering is visible without hiding session length. Skills rows are read-only and show recognized use count, optional Codex/CC sub-counts, and `last_used` recency; their controls add an all-time toggle plus All/Codex/CC provider badges without affecting the Now range selector. Per-mode SQL caps bound the payload: hosts 50, projects 100 (pre-subdir-merge), sessions 200 (passed from `useBreakdownData`'s `SESSION_BREAKDOWN_LIMIT`), and skills 100 (from `SKILL_BREAKDOWN_LIMIT`). For sessions whose rollup reports `has_subagents = true`, [[src/components/analytics/BreakdownPanel.tsx#SessionTreeBranch]] manages the per-row expand state and renders the lazy-fetched sub-agent tree through [[src/components/analytics/BreakdownPanel.tsx#SubagentRow]] — a recursive renderer depth-bounded by `SUBAGENT_MAX_DEPTH = 10` that uses [[src/hooks/useSessionSubagents.ts#useSessionSubagents]] for caching.
- **Insight cards**: `InsightCard` (generic), `SessionHealthCard`, `ProjectFocusCard`, `LearningProgressCard` — each shows a metric with trend arrow and sparkline. `InsightCard` also accepts an optional `description` prop that renders a top-right `?` help button and a sibling `.insight-card-tooltip` span; the [[features#Analytics Dashboard#Now Tab]] right-column context-savings cards opt into this for in-place metric explanations.
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
- **FloatingRunsWindow** (146 lines) — Collapsible OS window for run history, positioned with physical screen coordinates and owned by the Learning toggle so Strict Mode remounts and manual closes keep state synchronized.

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

`useIntegrations` in [[src/hooks/useIntegrations.ts]] loads provider statuses plus the persisted indicator primary provider, listens for `integrations-updated` and `indicator-updated`, and tracks per-provider in-flight actions.

It drives the [[features#Settings Window]]'s Integrations tab and blocked-window gating. The `enableProvider` function accepts an optional `apiKey` argument used by service-only providers like MiniMax, while `saveIndicatorPrimaryProvider` persists the status-indicator preference without introducing a separate frontend polling path. `rescan` invokes the `rescan_integrations` IPC and tracks `rescanInFlight` so the "Rescan PATH" row can spin while the backend re-derives the login-shell PATH and re-runs detection.

### Settings Hooks

Four hooks back the [[features#Settings Window]]: each owns one slice of state, calls Tauri IPC for mutations, and subscribes to the matching push event so multiple open Settings windows stay in sync.

| Hook | File | Source of truth | Listens for |
|------|------|-----------------|-------------|
| `useIntegrationFeatures` | [[src/hooks/useIntegrationFeatures.ts]] | `IntegrationFeatures` global flags (context preservation, activity tracking, context telemetry) | `integration-features-updated` |
| `useRuntimeSettings` | [[src/hooks/useRuntimeSettings.ts]] | `RuntimeSettings` background-task tunings (live-usage interval, plugin-update interval, rule watcher, always-on-top) | `runtime-settings-updated` |
| `useLearningSettings` | [[src/hooks/useLearningSettings.ts]] | `LearningSettings` (trigger mode, periodic interval, thresholds) | None — read on mount and after save |
| `useUiPrefs` | [[src/hooks/useUiPrefs.ts]] | `UiPrefs` localStorage values (layout mode, time mode, panel visibility) | `ui-prefs-updated` (frontend-emitted across windows) |

`useIntegrationFeatures` exposes typed setters per flag that each invoke a dedicated `set_*_enabled` IPC, while `useRuntimeSettings` and `useLearningSettings` save the whole struct in one call. `useUiPrefs.update(patch)` writes localStorage and emits `ui-prefs-updated` so the main window's [[src/App.tsx]] re-applies layout / time-mode / panel-visibility without a reload.

### Data Fetching Hooks

Hooks that invoke Tauri commands and return async state (data, loading, error).

| Hook | Returns | Tauri Commands |
|------|---------|----------------|
| `useAnalyticsData` | Snapshot count, loading, and error state for analytics empty-state gating | `get_snapshot_count` |
| `useLiveSummaryData` | Aggregate live `Sessions`, `Projects`, and range-scoped `Tokens` cards across enabled providers | `get_session_breakdown`, `get_token_history` |
| `useTokenData` | Token history with hostname/session filtering | `get_token_history`, `get_token_stats`, `get_token_hostnames` |
| `useCodeStats` | Lines added/removed by language | `get_code_stats`, `get_code_stats_history` |
| `useBreakdownData` | Host/project/session breakdown tables | `get_host_breakdown`, `get_project_breakdown`, `get_session_breakdown` |
| `useSessionHealth` | Avg duration, tokens, sessions/day with trend | `get_session_stats` |
| `useActivityPattern` | 24-hour hourly token distribution | `get_token_history` (derived) |
| `useLlmRuntimeStats` | Cumulative runtime, session count, turn count, avg per turn, sparkline | `get_llm_runtime_stats` |
| `useEfficiencyStats` | Tokens-per-LOC ratio with trend | Derived from token + code stats |
| `useVelocityStats` | LOC-per-hour with trend | Derived from code stats |
| `useLearningStats` | Rule counts by state, confidence buckets | `get_learned_rules` (derived) |
| `useLearningData` | Rules, runs, settings, observations, logs | Multiple learning commands + events |
| `useMemoryData` | Memory files, suggestions, projects | Multiple memory optimizer commands |
| `useSessionCodeStats` | Batch LOC stats per session (ref-cached) | `get_batch_session_code_stats` |
| `usePluginData` | Installed plugins, marketplaces, updates | Multiple plugin commands |
| `useCacheEfficiency` | Cache hit rate (derived from token history) | None (derived) |
| `useContextSavingsStats` | Context savings summary, time series, breakdowns, and recent events; subscribes to `context-savings-updated`. Powers both the [[features#Analytics Dashboard#Context Tab]] strip and the right column of [[features#Analytics Dashboard#Now Tab]]. | `get_context_savings_analytics` |
| `useSessionSubagents` | Per-`(provider, session_id)` lazy sub-agent tree state for the Sessions breakdown's expandable rows; caches results so collapse/re-expand never refetches | `get_session_subagent_tree` |

`useLiveSummaryData` fetches provider-filtered token and session history on demand so the top workload rail can aggregate `Sessions`, `Projects`, and range-scoped `Tokens` across whichever providers are enabled, while the grouped row sections continue to consume the already-fetched `UsageData` snapshot from `fetch_usage_data`.

The analytics hooks for the `Now` tab subscribe to backend push events instead of relying only on the 60-second polling fallback. `useCodeStats`, `useLlmRuntimeStats`, and `useBreakdownData` refresh on `sessions-index-updated`, while `useCodeInsights` refreshes on both `sessions-index-updated` and `tokens-updated` because it combines code and token history.

`useMemoryData` tracks concurrent optimization runs by run id and uses background refreshes for event-driven updates so `Optimize All` does not drop out of the running state or flash the all-projects view on every completion event. The hook initializes the Memories tab to the aggregate `__all__` selection on first load, then reuses the project-scoped delete IPC command to support current-view bulk deletion in both single-project and all-projects modes.

### State Pattern

Hooks follow a consistent async state pattern: `useState` for data/loading/error, `useRef` for initial load tracking, `useEffect` for fetching, periodic interval refresh, and Tauri event listener cleanup.

### Context

React Context providers used across the frontend for shared state.

- **ToastProvider** (`src/hooks/useToast.tsx`) — Notification system via React Context. Provides `toast(level, message)` to any component.
- **ChartCrosshairContext** (`src/components/analytics/ChartCrosshairContext.tsx`) — Synchronizes crosshair position across multiple Recharts charts.

## Type Definitions

[[src/types.ts]] contains shared TypeScript types mirroring the Rust models in [[src-tauri/src/models.rs]].

Key type categories: usage/token tracking (`UsageBucket`, `TokenDataPoint`, `TokenStats`, `ProviderCredits`), context savings (`ContextSavingsAnalytics`, `ContextSavingsEvent`), indicator state (`IndicatorPrimaryProvider`, `IndicatorMetric`, `StatusIndicatorState`), analytics (`BucketStats`, `SessionHealthStats`, `ResponseTimeStats`), learning (`LearnedRule`, `LearningRun`, `LearningSettings`), session search (`SearchHit`, `SearchResults`, `SessionContext`), plugins (`InstalledPlugin`, `Marketplace`, `PluginUpdate`), restart (`ClaudeInstance`, `RestartStatus`), memory (`MemoryFile`, `OptimizationSuggestion`).

Display enums: `TimeMode`, `RangeType`, `TrendType`, `BreakdownMode`, `SortMode`, `AnalyticsTab`, `PluginsTab`.

## Styling

Pure CSS with no framework. Dark theme (`#121216` background, `#d4d4d4` text, 11px system sans-serif).

### Stylesheets

Per-window CSS files under `src/styles/`, each scoped to a specific feature domain.

| File | Lines | Scope |
|------|-------|-------|
| `src/styles/index.css` | 3,798 | Global styles, main window, analytics, layout toggle |
| `src/styles/learning.css` | 940 | Learning window and components |
| `src/styles/sessions.css` | 498 | Session search window |
| `src/styles/plugins.css` | 847 | Plugin manager |
| `src/styles/restart.css` | 356 | Restart window |

### Color System

Semantic color palette used across all stylesheets for consistent status indication.

- Green `#34d399`: success, utilization < 50%
- Yellow `#fbbf24`: warning, utilization 50-80%
- Red `#f87171`: error, utilization >= 80%
- Blue `#60a5fa`: accents, interactive elements
- Memory type badges: blue (user), red (feedback), green (project), yellow (reference), purple (claude-md)
- Context savings categories: green (capture), blue (source), amber (router), purple (decision), pink (provider) — derived from the event-type prefix in [[src/components/analytics/ContextSavingsTab.tsx#categoryColor]] and reused by KPI swatches, breakdown dots, and event-line dots

### Responsive Scaling

The CSS variable `--s` scales all dimensions based on container size. Per-layout window sizes persist in localStorage (`quill-size-{live|analytics|both}`).

[[src/App.tsx]] now measures the rendered live content height at `--s: 1` before choosing the next scale, then applies a second fit-to-height correction pass against the actual post-scale `scrollHeight`. A `MutationObserver` on the live subtree retriggers that sizing pass when async live widgets change after the initial provider fetch, and split-ratio changes rerun the same fit logic with a small height gutter so the last usage row stays visible.

The live pane keeps the same `--s`-driven scaling strategy, but its summary rail is now provider-agnostic: the top card grid auto-fits whichever Claude and Codex buckets are present, while the grouped row sections below continue to shrink with the split divider instead of relying on a fixed Claude-only baseline. In split mode the main window stylesheet allows vertical scrolling in the live pane as a fallback, and the fit calculation reserves a bottom gutter above the divider so the last usage row does not sit flush against the resize bar.

## Utilities

Shared formatting and chart helper functions under `src/utils/`.

| File | Exports |
|------|---------|
| `src/utils/format.ts` | `formatNumber()` (thousand separators), `formatDurationSecs()` (human-readable) |
| `src/utils/tokens.ts` | `formatTokenCount()` (1.2M, 5.4k display) |
| `src/utils/time.ts` | `timeAgo()` (ISO string to relative "5m ago") |
| `src/utils/chartHelpers.ts` | `formatTime()`, `dedupeTickLabels()`, `anchorToNow()`, `getAreaColor()` |
| `src/utils/providers.ts` | `providerLabel()`, `normalizeProviderScope()`, `providerFilterLabel()`, `providerBadgeClass()` |
