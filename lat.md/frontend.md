# Frontend

The React 19 frontend is a multi-window Tauri application with custom hooks for IPC data fetching, Recharts for visualization, and pure CSS for styling.

## Entry Point

[[src/main.tsx]] routes to window-specific components based on the `?view=` URL parameter.

Each window gets its own Suspense boundary with a fallback. Per-window zoom persistence is stored in localStorage (`quill-zoom-{view}`) and supports Ctrl+/-, Ctrl+0 with a 0.5-2.0x range via Tauri's native webview zoom API, falling back to CSS `zoom` only outside Tauri. Ctrl+F is blocked to prevent the webview's native find-in-page (no search UI exists). A `ToastProvider` context wraps all views for notifications, [[src/hooks/useIntegrations.ts]] gates provider-dependent secondary windows when no provider is enabled, and [[src/windows/SessionsWindowView.tsx]] refreshes the session index on demand before loading search facets.

### Window Routes

Three Tauri windows are routed by the `?view=` URL parameter, each with its own Suspense boundary: the main split-pane app, the consolidated Manage workspace, and the release-notes viewer.

| Route | Component | Purpose |
|-------|-----------|---------|
| `?view=main` (default) | [[src/App.tsx]] | Split-pane live + analytics |
| `?view=manage` | [[src/windows/ManageWindowView.tsx]] | Rail-navigated Manage workspace; the five tool UIs (Sessions, Learning, Plugins, Instances, Settings) embedded as sections |
| `?view=release-notes` | `ReleaseNotesWindow` | Browse published GitHub release notes |

The former per-tool windows (`sessions`, `learning`, `plugins`, `restart`, `runs`, `settings`) were retired into Manage sections, and run history folded into the Learning section. All three remaining routes are reachable without an enabled provider — the Manage workspace gates each tool section inline (Settings always renders), so the former `BlockedWindow` per-window provider-blocking was removed.

## Manage Workspace

`?view=manage` ([[src/windows/ManageWindowView.tsx]]) is a single rail-navigated window that consolidates the former tool windows into the "Systems Pages" half of the monitor-vs-manage split. It is opened from the PFD titlebar's un-gated Tools button.

The left rail has five flat sections — Sessions, Learning (Rules / Memory / Runs), Plugins, Instances (instance restart), and Settings — with a signal-blue active indicator and a footer "Live" affordance back to the PFD. The active section persists to `localStorage` (`quill-manage-section`) and accepts a `?section=` deep-link (the titlebar cog opens `manage` at Settings). It uses the roomier Systems-Pages density and the Glass Cockpit tokens from DESIGN.md. Each section's content reuses the tool's existing window-view component, lazy-loaded and rendered with its own window chrome (titlebar/close) suppressed via `manage.css`; provider-dependent sections (Sessions, Learning, Plugins, Instances) show an inline no-provider state while Settings stays reachable. The Learning section's Runs toggle opens run history as an inline right-docked panel (folded in from the former floating `runs` window). The standalone tool windows, their `?view=` routes, and capabilities entries have been retired. A rail Search affordance and `⌘K` / `Ctrl K` open the [[src/components/CommandPalette.tsx]] — a substring-filtered list of the five sections plus Back-to-Live and Close-Tools actions, navigated with arrow keys and Enter. The titlebar, its launcher button, and the palette's Close action display the label "Tools"; the window label, `?view=manage` route, `manage.css`, and component names are unchanged.

## Browser Mock Mode

In a plain browser during dev (no Tauri runtime), the app installs a mock IPC layer so it renders with fixture data instead of failing every `invoke()`. This is what lets `/impeccable live` drive the real app in a browser.

[[src/main.tsx]] checks `import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)` before any IPC runs and only then dynamically imports [[src/mocks/installBrowserMock.ts#installBrowserMock]]. The dynamic import plus the `DEV` guard keep the mock and its fixtures out of production builds entirely.

[[src/mocks/installBrowserMock.ts#installBrowserMock]] calls `mockWindows` and `mockIPC` from `@tauri-apps/api/mocks`, routing every `invoke()` to [[src/mocks/ipcFixtures.ts#handleInvoke]], and adds a fixed `MOCK DATA` badge. [[src/mocks/ipcFixtures.ts#handleInvoke]] returns typed sample data for the data commands (provider statuses with an enabled provider so the dashboard is not gated, usage buckets spanning the green/amber/red thresholds, token/code/breakdown/analytics datasets), benign defaults for Tauri core `plugin:*` commands so `listen()` resolves with events left inert, and `null` for anything unmapped.

Model analytics mock handlers validate the same range, provider, and provider-qualified selection arguments as Tauri before applying dev-only failures. `modelFixture` selects lifecycle and exact empty-scope responses; retry keeps that scenario pending until another scenario or reload resets it. `modelFailure` rejects aggregate, history, session-page, session-detail, retry, or all commands through the shared structured envelope, and invalid nonempty controls warn and reject instead of silently falling back. Provider-qualified suppressed sources are removed before global or scoped facts. Opaque IDs remain dynamic evidence, not a support catalog.

Selected-model fixtures derive capped pages from observation data, including more than 20 matching sessions. Their opaque keyset cursor binds request identity to the final row's stable activity/provider/session tuple, rejects malformed, foreign, or stale anchors, and seeks past that tuple without offset drift. Lazy detail preserves parent/subagent boundaries, model gaps, repeated-model compression, Unicode-scalar primary ties, and one page-to-detail deletion returning bounded `not_found`.

A dev-only Vite plugin in [[vite.config.ts]] (`apply: "serve"`) relaxes the strict production CSP so the browser can load Vite HMR, React Fast Refresh, and the Impeccable live client at `http://localhost:8400`. Because it is serve-only, `vite build` never runs it and the shipped CSP is untouched.

## Main Window Layout

[[src/App.tsx]] implements a split-pane layout with a draggable divider separating the [[features#Live Usage View]] and [[features#Analytics Dashboard]].

The layout supports two orientations controlled by a `LayoutMode` toggle (`"stacked"` or `"side-by-side"`) persisted in localStorage as `quill-layout-mode`. Stacked mode (default) places Live above Analytics with a horizontal divider; side-by-side mode places Live on the left and Analytics on the right with a vertical divider. Each orientation has an independent split ratio (0.15-0.85) persisted separately (`quill-split-ratio` for stacked, `quill-split-ratio-h` for side-by-side). The layout supports keyboard-driven resizing (ArrowUp/Down for stacked, ArrowLeft/Right for side-by-side), pointer-anchored divider dragging, and window resize events. Usage data refreshes every 3 minutes via `fetch_usage_data()` only while Live is visible and a provider is active. Provider loading and setup states replace only Live content, so the transcript-backed Analytics pane stays mounted without enabled live providers. Manual Refresh updates integration status; the Live effect owns the resulting usage fetch after that status settles, preventing duplicate requests. Right-click also exposes Quit.

The split panes keep their root content views shrinkable with `min-width: 0` on `UsageDisplay` and `AnalyticsView`, preventing intrinsic child widths from forcing the macOS window wider when switching orientations.

The shared `.content` class centers its children for full-pane loading and empty states, so each pane wrapper (`.live-content`, `.analytics-content`) overrides `align-items` to `stretch` and `justify-content` to `flex-start`. Without this override the pane re-centers vertically whenever its child changes height (for example when switching the Context tab range), which the user perceives as the page jumping to the middle even though `scrollTop` is unchanged.

### Component Tree

The main window nests `TitleBar` at the top, `UsageDisplay` and `AnalyticsView` in the panels area.

`TitleBar` has feature buttons on the left, a centered static `QUILL` brand label, and a right-side cluster with the version button followed by a settings button (sliders icon, immediately right of the version) that opens the standalone [[features#Settings Window]], then the close control. An active Live pane remains dismissible during provider discovery or when no live provider is enabled; only reopening hidden Live is gated. Analytics remains independently available for transcript-backed data. `UsageDisplay` shows live rate limit buckets. [[src/components/analytics/AnalyticsView.tsx#AnalyticsView]] renders tabbed analytics with a matching labeled `tabpanel` shell for every visible tab. In stacked mode, Live is above Analytics; in side-by-side mode, Live is on the left and Analytics on the right.

## Components

Components are organized by feature domain under `src/components/`.

### Core Components

Top-level UI chrome and live rate limit display shared across the main window.

- **TitleBar** (`src/components/TitleBar.tsx`) — Custom window chrome with a left-aligned Live/Analytics toggle plus a single un-gated **Tools** button (labeled "Tools") that opens the [[lat.md/frontend#Frontend#Manage Workspace]] window (it replaced the former Learning/Search/Plugins/Restart launch icons), a centered static `QUILL` brand label, and a right-aligned cluster containing the version button followed by a settings button rendered as a horizontal-sliders icon (immediately right of the version) that opens the workspace at its Settings section, then the close control. When the frontend's periodic updater check finds a release, it also shows an `Update x.y.z` action that installs via [[src-tauri/src/lib.rs#install_app_update]] so the backend owns the restart handoff. The version label is rendered as a button that opens the `release-notes` window via [[src/windows/ReleaseNotesWindow.tsx]]. Owns the confirmation-driven enable/disable flow via `ConfirmDialog`.
- **ReleaseNotesWindow** (`src/windows/ReleaseNotesWindow.tsx`) — Standalone window that fetches published GitHub releases through the [[src-tauri/src/lib.rs#get_release_notes]] command, shows the latest first, and places Previous/Next navigation plus the selectable release URL in a top toolbar below the titlebar. Centers the release tag between the release counter and publish date, renders release bodies as sanitized GitHub-flavored Markdown that fills the scroll area, surfaces loading, empty, and error states with a Retry control, and supports Escape plus Left/Right arrow keyboard navigation.
- **ProviderMenu** (`src/components/integrations/ProviderMenu.tsx`) — Reusable provider action panel rendered as a compact terminal-utility list of 22 px rows separated by 1 px hairlines. Inline rows for Layout (stacked/side-by-side icon toggle), Status (compact `<select>` for the indicator primary provider), and Context (working-context preservation toggle) come first, followed by the `Integrations` group (Claude Code, Codex, MiniMax) and then `Brevity` (Claude Code, Codex). The Integrations group leads with a "Rescan PATH" row whose `RUN`/`...` toggle calls the `rescan` callback (which invokes the `rescan_integrations` IPC) so users can re-derive the login-shell PATH after installing a CLI or editing shell config without restarting. Each provider toggle is a single 36 px-min `pmenu-toggle` pill that resolves to one of `ON` / `OFF` / `N/A` / `SETUP` / `…` / `—` depending on `inFlightProviders`, `setupState`, `detectedCli`, and per-provider `enabled` flags, with semantic colors drawn from [[lat.md/frontend#Frontend#Styling#Color System]] (green = on, dim = off, red = unavailable, yellow = needs setup, blue = busy). Hovering any row in a section instantly shows a detailed `pmenu-tooltip` (one of `layout`, `status`, `context`, `brevity`, `integrations`) rendered via `react-dom/createPortal` into `document.body` to escape the popover's `overflow-y: auto`; the tooltip is positioned `fixed` to the left of the menu by default and falls back below the popover when there is no horizontal room for the 252 px panel, with a small CSS-rotated diamond pointing back at the source row. Tooltip copy lines support inline `<code>` rendering via a backtick parser. When a provider row shows N/A and `lastDetectionAttempts` is non-empty, hover replaces the generic Integrations tooltip with a per-row diagnostic listing every path Quill checked while looking for that provider's CLI. The portal layer dismisses on `mouseleave`, window resize, or menu scroll. Layout props remain optional for backward compatibility with the legacy `IntegrationsWindowView`.
- **ConfirmDialog** (`src/components/ConfirmDialog.tsx`) — Shared confirmation modal used for destructive provider cleanup and provider installation confirmation.
- **IntegrationsWindowView** (`src/windows/IntegrationsWindow.tsx`) — Legacy standalone window host for `ProviderMenu` (unused since inline popover migration).
- **UsageDisplay** (`src/components/UsageDisplay.tsx`) — Composes the shared workload summary rail, grouped provider limit sections, the detailed-row time mode selector, and provider-error handling for the main window's live pane.
- **LiveSummaryModule** (`src/components/live/LiveSummaryModule.tsx`) — Shared top-of-pane workload module with the 1h/6h/12h/24h selector, freshness label, and aggregate `Sessions`, `Projects`, and `Tokens` cards across the enabled providers.
- **ProviderUsageModule** (`src/components/live/ProviderUsageModule.tsx`) — Reusable provider section that renders quota rows with a provider badge and source note. For MiniMax, filters buckets to primary models (M\*, coding-plan-search, coding-plan-vlm) and shows an "All models" hover badge with a tooltip displaying the remaining models' name, utilization, and reset countdown.
- **UsageRow** (`src/components/UsageRow.tsx`, 243 lines) — Individual rate limit visualization with three display modes: pace marker (vertical line), dual bars (time elapsed vs utilization), or background fill. When its `resets_at` is already in the past (countdown reads "now"), the row renders as stale — muted percentage, no severity badge, and a neutral slate bar in every mode — so a value from a bygone window never reads as live severity. Exports `formatCountdown` and `gradientColor` utilities for reuse by tooltip renderers.

### Analytics Components

Analytics components in `src/components/analytics/` provide Now, Trends, Charts, Models, and an optional Context tab.

- **NowTab** (214 lines) — Real-time metrics with range selector (1h/24h/7d/30d), six insight cards, a 24-hour activity heatmap, and a switchable breakdown panel (sessions/projects/hosts/skills).
- `NowTab` shares one comparison-range code-history fetch between the efficiency and velocity cards via `src/hooks/useCodeInsights.ts`, which avoids firing the same `get_code_stats_history` IPC call twice per refresh.
- Selecting a session in `NowTab` now keeps provider identity alongside `session_id`, so token charts, compact token stats, and delete actions stay scoped to the correct Claude or Codex session.
- **TrendsTab** (105 lines) — Token trends, code velocity, and cache efficiency charts with week-over-week comparison.
- **ChartsTab** (454 lines) — Composite Recharts chart with three axes (utilization, tokens, LOC). Lazy-loaded with Suspense.
- **TabBar** — Analytics' horizontally scrollable underline navigation keeps Models available alongside Now, Trends, Charts, and optional Context. It uses stable tab/panel IDs plus roving Arrow/Home/End keyboard focus and activation.
- **ModelBackfillStatus** — [[src/components/analytics/models/ModelBackfillStatus.tsx#ModelBackfillStatus]] keeps root discovery and source processing counts separate, labels every non-complete state as incomplete, distinguishes enumerated inventories with source failures from unavailable roots, and exposes partial/failed Retry plus atomic polite announcements without hiding recovered data.
- **ModelUsageHistory** — [[src/components/analytics/models/ModelUsageHistory.tsx#ModelUsageHistory]] renders fixed-range, stacked attributed/unattributed token buckets with a provider-neutral signal-blue selected-model overlay. It preserves loaded history during refresh failures and exposes every bucket bound and series value through a visually hidden semantic table.
- **ModelUsageTable** — [[src/components/analytics/models/ModelUsageTable.tsx#ModelUsageTable]] renders every provider-qualified raw model as a copyable, bidi-isolated row with provider-only color, complete token/session evidence, and deterministic sorting across all columns. Its default token-descending order resolves ties by provider and raw ID using Unicode-scalar comparison; memoized formatted row views isolate selection and live-status renders, while retained rows remain usable beside request-local Retry errors.
- **ContextSavingsTab** — Context preservation analytics with a four-column stats strip (saved, indexed, returned, routing) over a stacked trend chart, breakdown table, and recent events feed. Breakdown rows render a relative-magnitude bar fill behind each row scaled to the largest event count, and recent events use a single-line log format with category swatches and a directional byte arrow (→ indexed, ← returned). Confidence is hidden for exact estimates. `AnalyticsView` shows this tab when context preservation is enabled or historical context-savings events exist; a persisted active Context tab remains mounted while that status is unresolved and resets only after a successful status read proves it unavailable.
- **UsageChart** (456 lines) — `ComposedChart` with Area, Line, and custom Tooltip. Uses `ChartCrosshairContext` for tooltip synchronization.
- **BreakdownPanel** — Sortable table showing sessions, projects, hosts, or skills with compact count columns. It renders all rows in a flexing scroll area that fills the available analytics pane height instead of paginating the breakdown. Session rows display provider badges and use provider-safe composite keys for selection. Hosts and projects show `<recency>` in their time column (e.g. `2h ago`); sessions show `<recency> · <duration>` (e.g. `23h ago · 23h 43m`, or `active · 6m` when `last_active` is within the last 5 minutes), so the SQL `last_active DESC` ordering is visible without hiding session length. Skills rows show recognized use count and `last_used` recency — provider breakdown lives in the filter strip rather than inline on each row, so the count column stays uncluttered; their controls render on a dedicated row directly beneath the breakdown mode tabs and intentionally use a different visual vocabulary than the chunky `.range-tab` container pills above: an underline-indicator text filter strip (`All / Codex / Claude`) sits left-aligned, and a right-justified outlined uppercase `∞ ALL TIME` chip toggles the all-history scope. A Skills-only header row labels Skill, Uses, and Last used as small sort buttons; the default is Uses descending, and clicking the active title flips direction without refetching from Tauri. The three shape languages (container pills, underline filters, outlined glyph chip) keep each control reading as its own thing instead of three stacked rows of identical buttons, and the Skills-specific filters never crowd the mode tabs or affect the Now range selector. Every skill row renders the shared tiny hairline disclosure caret and lazy-fetches per-(project, hostname) counts via [[src/hooks/useSkillProjects.ts#useSkillProjects]] when opened, including rows whose `project_count` is `1`; the drilldown renders indented sub-rows below the parent skill and labels null-project rows as `No project data` so child counts still sum to the parent. Sub-rows reuse the sub-agent tree-guide and indent CSS for visual consistency with session→sub-agent drilldowns and carry a dedicated `breakdown-row-skill-project` class for future styling overrides. Switching filter scope (provider/all-time) collapses every expanded skill so stale sub-rows cannot survive a filter change. Per-mode SQL caps bound the payload: hosts 50, projects 100 (pre-subdir-merge), sessions 200 (passed from `useBreakdownData`'s `SESSION_BREAKDOWN_LIMIT`), and skills 100 (from `SKILL_BREAKDOWN_LIMIT`). For sessions whose rollup reports `has_subagents = true`, [[src/components/analytics/BreakdownPanel.tsx#SessionTreeBranch]] manages the per-row expand state and renders the lazy-fetched sub-agent tree through [[src/components/analytics/BreakdownPanel.tsx#SubagentRow]] — a recursive renderer depth-bounded by `SUBAGENT_MAX_DEPTH = 10` that uses [[src/hooks/useSessionSubagents.ts#useSessionSubagents]] for caching; non-expandable session rows omit the disclosure slot so their ids stay flush with normal row padding.
- **Insight cards**: `InsightCard` (generic), `SessionHealthCard`, `ProjectFocusCard`, `LearningProgressCard` — each shows a metric with trend arrow and sparkline. `InsightCard` also accepts an optional `description` prop that renders a top-right `?` help button and a sibling `.insight-card-tooltip` span; the [[features#Analytics Dashboard#Now Tab]] right-column context-savings cards opt into this for in-place metric explanations.
- **Sparklines**: `TokenSparkline`, `CodeSparkline`, `MiniChart` — small inline Recharts charts.
- **Utility**: `TabBar`, `TogglePills` (range selector), `ActivityHeatmap`, `CompactStatsRow`, `shared.tsx` (getColor, TrendArrow).

### Models Composition

`ModelsTab` coordinates range, provider, and provider-qualified model selection without treating raw model identifiers as product configuration.

The user-facing metrics and investigation contract is summarized in [[features#Analytics Dashboard#Models Tab]].

[[src/components/analytics/ModelsTab.tsx#ModelsTab]] receives range state from Analytics routing and derives provider controls from aggregate responses. While a new scope masks aggregate data, it preserves only the already selected, response-derived provider so filter chrome matches the in-flight IPC scope; fresh evidence replaces that temporary option and layout-time reconciliation clears provider or model selection before paint when no longer represented. Summary, history, and table regions retain independent loading surfaces; aggregate and history failures expose request-local Retry actions without replacing unaffected same-scope data. Provider changes clear an incompatible model immediately, while model-only selection leaves the aggregate request identity unchanged and refetches history only.

The retained-history status is independent from aggregate and history request errors. A final empty claim requires persisted inventory completeness, clean complete backfill counters, and backend `scopeFinal`; it then applies global-session, filtered-session, and reliable-evidence precedence. Pending, running, partial, failed, or retrying history instead labels the scope provisional or incomplete while leaving recovered summary, history, and table data mounted. Backend scope facts already exclude suppressed sources, so the frontend never reconstructs emptiness from visible rows.

Selecting a model mounts the session panel with both detail hooks consuming the same frontend refresh generation. Paging and expanded histories refresh independently. After a bounded `not_found` notice, composition hides only that exact provider/session row for the active range and model identity. Old-scope callbacks are ignored; a successful page snapshot that omits the row clears the local hide marker so later valid reappearance remains possible.

[[src/components/analytics/AnalyticsView.tsx#AnalyticsView]] owns the Models range independently from snapshot-backed ranges and restores Models from persisted tab state. Every visible tab keeps a lightweight panel shell using the stable IDs exported by [[src/components/analytics/TabBar.tsx#analyticsTabId]] and [[src/components/analytics/TabBar.tsx#analyticsPanelId]]. Models content is lazy until first visit, then remains mounted under that shell across internal Analytics tab switches; the hidden panel preserves its request state, fixed-window event listener, fallback poll, filters, and selection without preloading Models or repeating listener-gap reconciliation on each opening. Other data-heavy content mounts only for the effective active panel. Definitively unavailable Context state maps immediately to Now before the persistence effect runs. Snapshot polling mounts only with active Now, Trends, or Charts content; [[src/hooks/useAnalyticsData.ts#useAnalyticsData]] marks snapshot-count readiness only after a successful count response, so the shared empty state cannot mistake the default zero during failure recovery for confirmed absence. Models and Context remain independent from snapshot requests and failures.

### Model Session Detail Panel

`ModelDetailPanel` presents selected-model session paging and lazy chain history while keeping asynchronous state in its hooks.

[[src/components/analytics/models/ModelDetailPanel.tsx#ModelDetailPanel]] owns only the open disclosure keys. Native buttons expose stable controlled panel IDs; page replay, pagination, row refresh, retained errors, and stale-session notices remain independently visible. Each expanded session shows provider-qualified identities, range-scoped totals, parent/subagent metadata, and backend-ordered model or identity-gap segments without interpreting raw identifiers.

Compact hairline-divided rows collapse at narrow analytics widths while preserving keyboard focus, row-local status, identifier overflow handling, chain hierarchy, and model-gap visibility.

### Learning Components

Rule management and memory optimization UI in `src/components/learning/`.

- **MemoriesPanel** (807 lines) — Memory optimization UI with project selector, file browser with content preview, suggestion approval/denial, and custom project management. The largest frontend component.
- **RuleCard** — Displays a learned rule with name, confidence %, and a metadata row (domain, source, project, current operator-feedback verdict) in muted text. Every rendered rule exposes operator-feedback actions (accept / reject = optimistic single click; bad = the existing two-step inline confirm, identical in shape to the promote confirm) threaded via `useLearningData.submitRuleFeedback` (feature 005 US3 / R-5). Active rules (on disk, non-terminal lifecycle): no state badge, feedback + delete. Discovered rules (DB-only): lifecycle badge, promote button with inline two-step confirmation, feedback, and expandable DB-stored content preview. On-disk rules in a terminal lifecycle (`superseded`, `conflict_flagged`, `rejected`, `tombstoned`, `suppressed`, `invalidated`) render a distinct lifecycle badge and group with discovered, never as active. When the rule's normalized `provider_scope` spans more than one provider the shared scope badge carries an inline `ⓘ` disclosure (verbatim provider-asymmetry copy from [[src/utils/providers.ts#PROVIDER_ASYMMETRY_DISCLOSURE]] via `title`/`aria-label`) — Codex is captured for Bash/shell only, so shared rules are structurally Claude-weighted (feature 005 R-7 / M-6 / FR-028); single-provider badges show no disclosure.
- **SuggestionCard** (258 lines) — Memory optimization suggestion with approve/deny/undo actions and diff summaries.
- **StatusStrip** — Observation count, unanalyzed count, last run time, and "Run Analysis" button. On the combined "All Providers" scope only, when at least one shared-scope rule exists, it renders the quantified provider-asymmetry disclosure ([[src/utils/providers.ts#PROVIDER_ASYMMETRY_DISCLOSURE]]) appended with a per-provider shared-rule contribution count derived in `LearningWindow` from the already-fetched rules' `provider_scope` (no extra fetch); single-provider filters omit the note (feature 005 R-7 / M-6 / FR-028).
- **DomainBreakdown** (38 lines) — Rules-by-domain pie chart.
- **RunHistory** — Run list with status badges and per-phase breakdown. The selected-run detail block surfaces the derived `LearningRun.inference` rollup ([[src/types.ts#RunInferenceSummary]]) as Model / Cost / Inference-time rows (em-dash and never a crash when `inference` is absent on legacy/micro runs), plus a Failed-calls row when any inference call failed; the existing wall-clock Duration row is kept alongside the summed inference time. `degraded` is a first-class status with a distinct amber ⚠ icon and phase dot (no longer masked by the hard-fail ✗) and a degraded-but-with-rules result label. A presentational consecutive-failure banner (no circuit-breaker, no extra fetch) appears when the last K=3 terminal-with-verdict runs are all hard `failed` (`running`/`interrupted` neither contribute nor reset) (feature 005 R-7 / H-6 / L-3 / FR-024). Rendered inline as a right-docked panel within the Learning section (toggled by the toolbar Runs button), reusing the same `runs`/`liveLogs` from [[src/hooks/useLearningData.ts]]; the former standalone floating run-history window was retired.

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
| `useSkillProjects` | Per-`(skill_name, requestKey)` lazy project-breakdown state for the Skills breakdown's expandable rows; `requestKey` encodes `${mode}:${days}:${allTime}:${provider}` so cache slots invalidate on filter change while strictly lazy-fetching only on expand | `get_skill_project_breakdown` |
| `useModelSessions` | Selected provider-qualified model paging with atomic shared-refresh replay and operation-local recovery | `get_model_sessions` |
| `useSessionModelHistory` | Per-`(provider, sessionId, range)` lazy model-chain history for expanded Models rows; shared refresh refetches expanded rows, invalidates collapsed caches, and preserves good data behind row-local errors | `get_session_model_history` |

`useLiveSummaryData` fetches provider-filtered token and session history on demand so the top workload rail can aggregate `Sessions`, `Projects`, and range-scoped `Tokens` across whichever providers are enabled, while the grouped row sections continue to consume the already-fetched `UsageData` snapshot from `fetch_usage_data`.

The analytics hooks for the `Now` tab subscribe to backend push events instead of relying only on the 60-second polling fallback. `useCodeStats`, `useLlmRuntimeStats`, and `useBreakdownData` refresh on `sessions-index-updated`, while `useCodeInsights` refreshes on both `sessions-index-updated` and `tokens-updated` because it combines code and token history.

`useMemoryData` tracks concurrent optimization runs by run id and uses background refreshes for event-driven updates so `Optimize All` does not drop out of the running state or flash the all-projects view on every completion event. The hook initializes the Memories tab to the aggregate `__all__` selection on first load, then reuses the project-scoped delete IPC command to support current-view bulk deletion in both single-project and all-projects modes.

### State Pattern

Hooks follow a consistent async state pattern: `useState` for data/loading/error, `useRef` for initial load tracking, `useEffect` for fetching, periodic interval refresh, and Tauri event listener cleanup.

### Model Analytics Hook

`useModelAnalytics` keeps aggregate, history, and backfill retry state independent so one failed region cannot replace successful same-scope data from another.

[[src/hooks/useModelAnalytics.ts#useModelAnalytics]] masks old-scope responses immediately, rejects superseded request generations, and exposes separate initial-loading, refresh-loading, structured-error, and Retry state. Backfill status persists across scope changes from accepted aggregate snapshots and the structured retry response; generation, lifecycle, inventory, and monotonic progress outrank wall-clock timestamps so clock rollback cannot hide completion. Its own guarded Retry never clears recovered aggregate or history data. One Strict Mode-safe `model-analytics-updated` listener starts a fixed one-second deadline at the first event, then reconciles once when its asynchronous registration becomes active to close the initial fetch/subscription gap unless a captured event already owns that refresh; a disposed registration only unsubscribes. Same-identity signals that arrive during an aggregate or history request collapse into one deferred refresh after the settled request state commits, while an identity change still supersedes the old request immediately. A 60-second poll uses the same aggregate/history refresh path. Each external refresh advances a frontend-only generation for mounted detail hooks; changing only selected model refetches history without refetching aggregates.

### Model Session Detail Hooks

`useModelSessions` pages one selected provider-qualified model while keeping refresh recovery independent from aggregate and history requests.

[[src/hooks/useModelSessions.ts#useModelSessions]] resets immediately when range or exact model identity changes and stays idle without a selection. Initial, Load more, and replay operations expose separate structured errors and Retry actions. Load more appends after provider/session deduplication. A shared refresh replays sequentially from page one through the prior page count, requiring stable response identity, total, and opaque cursor progress before atomically swapping the page set. Failed replay keeps prior pages visible; stale Load more cursors or drifting snapshots recover through a fresh page-one replay. Request identities, logical epochs, and monotonic generations reject late or pre-refresh responses, while only the duplicate effects of one React Strict Mode logical request share an in-flight page call.

### Session Model History Hook

`useSessionModelHistory` owns lazy, row-local chain requests without coupling their lifecycle to selected-model page replay.

[[src/hooks/useSessionModelHistory.ts#useSessionModelHistory]] keys successful histories by provider, session, and range. Expanding is the only action that starts an initial request. Shared refresh discards collapsed caches and refetches each expanded row independently. A failed refresh or Retry retains its last accepted history; structured `not_found` becomes a distinct stale-row result for bounded reconciliation. Hook-global monotonic request tokens plus active-expansion, exact-scope, and response-identity guards reject late results even after a cache key is reused. Cancellation resets internal loading state without updating an unmounted component; Strict Mode effect setup replays canceled expanded rows. Active-token and cleanup-replay metadata are removed on settle, collapse, scope invalidation, or the next setup.

### Context

React Context providers used across the frontend for shared state.

- **ToastProvider** (`src/hooks/useToast.tsx`) — Notification system via React Context. Provides `toast(level, message)` to any component.
- **ChartCrosshairContext** (`src/components/analytics/ChartCrosshairContext.tsx`) — Synchronizes crosshair position across multiple Recharts charts.

## Type Definitions

[[src/types.ts]] contains shared TypeScript types mirroring the Rust models in [[src-tauri/src/models.rs]].

Key type categories: usage/token tracking (`UsageBucket`, `TokenDataPoint`, `TokenStats`, `ProviderCredits`), context savings (`ContextSavingsAnalytics`, `ContextSavingsEvent`), indicator state (`IndicatorPrimaryProvider`, `IndicatorMetric`, `StatusIndicatorState`), analytics (`BucketStats`, `SessionHealthStats`, `ResponseTimeStats`), learning (`LearnedRule`, `LearningRun`, `LearningSettings`), session search (`SearchHit`, `SearchResults`, `SessionContext`), plugins (`InstalledPlugin`, `Marketplace`, `PluginUpdate`), restart (`ClaudeInstance`, `RestartStatus`), memory (`MemoryFile`, `OptimizationSuggestion`).

Display enums: `TimeMode`, `RangeType`, `TrendType`, `BreakdownMode`, `SortMode`, `AnalyticsTab`, `PluginsTab`.

## Styling

Pure CSS with no framework, organized around a `:root` design-token layer in `src/styles/index.css` per DESIGN.md. Dark theme: near-black `--console-black` (`#121216`) canvas, `--readout` (`#d4d4d4`) text, 11px Geist with system fallback.

### Typography

Body/UI text is **Geist** and monospace contexts (ids, code, paths) are **Geist Mono** — both self-hosted variable fonts (weights 100–900) with system stacks as fallback.

Both are vendored from the `geist` npm package into `src/assets/fonts/` (`Geist-Variable.woff2`, `GeistMono-Variable.woff2`) and declared via `@font-face` in `index.css` with `font-display: swap`. Every window stylesheet's mono stack leads with `"Geist Mono"`.

### Design Tokens

The canonical palette lives as `:root` CSS custom properties in `src/styles/index.css`, following DESIGN.md. Because [[src/main.tsx]] loads `index.css` for every window, these tokens are global to all stylesheets.

Tokens cover backgrounds (`--console-black`, `--panel-deep`, `--panel-raised`, `--card-graphite`, `--slate-input`, `--graphite-line`), text (`--readout`, `--readout-bright`, `--label`, `--label-faint`), the status meter (`--meter-green` / `--meter-amber` / `--meter-red`), accents (`--signal-blue` / `--signal-cyan` / `--signal-violet` / `--signal-orchid`), provider identity (`--provider-claude` / `--provider-codex` / `--provider-minimax` / `--provider-agent`), and `--radius-*` / `--space-*` scales. Every window stylesheet reads its palette from these vars. The former Tokyo-night palette (plugin and restart windows), the divergent green and lifecycle colors (learning window), and the GitHub-dark insight-card/tooltip sub-palette plus assorted near-whites (`index.css`, `settings.css`) have all been unified onto the canonical tokens. The only remaining color literals are neutral white/black alpha — the dimming ladder — and one intentional lighter-green toggle-hover tint.

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

Semantic palette, drawn from the `:root` tokens. Status color is reserved; identity color is fixed per provider.

- **Status meter** (`--meter-green` `#34d399` < 50%, `--meter-amber` `#fbbf24` 50-80%, `--meter-red` `#f87171` >= 80%): utilization, trends, success/warning/error. Reserved for threshold state only.
- **Signal blue** (`--signal-blue` `#60a5fa`): accents, selection, focus rings, primary actions. The sessions search/filter focus and active-sort toggle use this — previously green, which collided with the meter.
- **Provider identity** — one fixed color per provider across all four surfaces (titlebar usage badges, breakdown tags, learning badges, session-search badges): Claude `--provider-claude` blue, Codex `--provider-codex` cyan, MiniMax `--provider-minimax` violet, sub-agent `--provider-agent` orchid. Drawn from the cool ramp so identity never reuses a status hue; the `shared` learning scope renders neutral.
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
