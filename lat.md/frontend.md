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
| `?view=manage` | [[src/windows/ManageWindowView.tsx]] | Rail-navigated Manage workspace; four tool UIs (Sessions, Learning, Instances, Settings) embedded as sections |
| `?view=release-notes` | `ReleaseNotesWindow` | Browse published GitHub release notes |

The former per-tool windows (`sessions`, `learning`, `restart`, `runs`, `settings`) were retired into Manage sections, and run history folded into the Learning section. All three remaining routes are reachable without an enabled provider — the Manage workspace gates each tool section inline (Settings always renders), so the former `BlockedWindow` per-window provider-blocking was removed.

## Manage Workspace

`?view=manage` ([[src/windows/ManageWindowView.tsx]]) is a single rail-navigated window that consolidates the former tool windows into the "Systems Pages" half of the monitor-vs-manage split. It is opened from the PFD titlebar's un-gated Tools button.

The left rail has four flat sections — Sessions, Learning (Rules / Memory / Runs), Instances (instance restart), and Settings — with a signal-blue active indicator and a footer "Live" affordance back to the PFD. The active section persists to `localStorage` (`quill-manage-section`) and accepts a `?section=` deep-link (the titlebar cog opens `manage` at Settings). It uses the roomier Systems-Pages density and the Glass Cockpit tokens from DESIGN.md. Each section's content reuses the tool's existing window-view component, lazy-loaded and rendered with its own window chrome (titlebar/close) suppressed via `manage.css`; provider-dependent sections (Sessions, Learning, Instances) show an inline no-provider state while Settings stays reachable. The Learning section's Runs toggle opens run history as an inline right-docked panel (folded in from the former floating `runs` window). The standalone tool windows, their `?view=` routes, and capabilities entries have been retired. A rail Search affordance and `⌘K` / `Ctrl K` open the [[src/components/CommandPalette.tsx]] — a substring-filtered list of the four sections plus Back-to-Live and Close-Tools actions, navigated with arrow keys and Enter. The titlebar, its launcher button, and the palette's Close action display the label "Tools"; the window label, `?view=manage` route, `manage.css`, and component names are unchanged.

## Browser Mock Mode

In a plain browser during dev (no Tauri runtime), the app installs a mock IPC layer so it renders with fixture data instead of failing every `invoke()`. This is what lets `/impeccable live` drive the real app in a browser.

[[src/main.tsx]] checks `import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)` before any IPC runs and only then dynamically imports [[src/mocks/installBrowserMock.ts#installBrowserMock]]. The dynamic import plus the `DEV` guard keep the mock and its fixtures out of production builds entirely.

[[src/mocks/installBrowserMock.ts#installBrowserMock]] calls `mockWindows` and `mockIPC` from `@tauri-apps/api/mocks`, routing every `invoke()` to [[src/mocks/ipcFixtures.ts#handleInvoke]], and adds a fixed `MOCK DATA` badge. [[src/mocks/ipcFixtures.ts#handleInvoke]] returns typed sample data for the data commands (provider statuses with an enabled provider so the dashboard is not gated, usage buckets spanning the green/amber/red thresholds, token/code/breakdown/analytics datasets), benign defaults for Tauri core `plugin:*` commands so `listen()` resolves with events left inert, and `null` for anything unmapped.

Widget fixtures answer per range rather than serving one fixed window. `get_provider_token_series` returns aligned per-provider bucket values with per-provider totals whose sum equals the response total by construction, and `get_activity_series` returns aligned session and project counts; both honor the `range` (including the new `6h` step) and `buckets` arguments. `get_token_history` and `get_code_stats_history` now vary their point spacing for the hour-granular ranges instead of always returning the 24h shape.

Model analytics mock handlers validate the same range, provider, and provider-qualified selection arguments as Tauri before applying dev-only failures. `modelFixture` selects lifecycle and exact empty-scope responses; retry keeps that scenario pending until another scenario or reload resets it. `modelFailure` rejects aggregate, history, session-page, session-detail, retry, or all commands through the shared structured envelope, and invalid nonempty controls warn and reject instead of silently falling back. Provider-qualified suppressed sources are removed before global or scoped facts. Opaque IDs remain dynamic evidence, not a support catalog.

Selected-model fixtures derive capped pages from observation data, including more than 20 matching sessions. Their opaque keyset cursor binds request identity to the final row's stable activity/provider/session tuple, rejects malformed, foreign, or stale anchors, and seeks past that tuple without offset drift. Lazy detail preserves parent/subagent boundaries, model gaps, repeated-model compression, Unicode-scalar primary ties, and one page-to-detail deletion returning bounded `not_found`.

A dev-only Vite plugin in [[vite.config.ts]] (`apply: "serve"`) relaxes the strict production CSP so the browser can load Vite HMR, React Fast Refresh, and the Impeccable live client at `http://localhost:8400`. Because it is serve-only, `vite build` never runs it and the shipped CSP is untouched.

## Main Window Layout

[[src/App.tsx]] is the widget shell: a fixed 360px window holding [[src/components/widget/WidgetTitleBar.tsx]], a hairline, and one scrolling content column. The split-pane layout and its draggable divider were replaced by the widget redesign.

The shell owns only the app-lifecycle work that has no other home: usage polling every 3 minutes via `fetch_usage_data()` while a provider is enabled, the four-hour updater check, the right-click Refresh/Quit menu, close-to-tray on both the titlebar control and the window manager's close request, and the window's height. Manual Refresh updates integration status; the polling effect owns the resulting usage fetch after that status settles, preventing duplicate requests. When no provider is enabled the column shows one shell-level empty state with a rescan action in place of every band.

Height is content-derived: a `ResizeObserver` on the content column asks the window for the measured height plus the 43px chrome (titlebar, hairline, and the shell's border), clamped to the 200-900px bounds declared in `src-tauri/tauri.conf.json`. Width is never touched. The measured element sits inside the scroll container and is never constrained by the viewport, so the measurement cannot oscillate, and only the column below the titlebar scrolls — which is what keeps the widget reachable under webview zoom or on a platform that refuses a programmatic resize.

### Component Tree

The widget nests [[src/components/widget/WidgetTitleBar.tsx#WidgetTitleBar]] above a scrolling content column holding [[src/components/widget/LimitsSection.tsx]] and, below it, the switchable view region ([[lat.md/frontend#Frontend#Components#Widget View Region]]).

`WidgetTitleBar` carries the widget's whole chrome contract in three grid tracks: the brand glyph and `QUILL` wordmark on the left, the update button centred on the window (rendered only once the updater check has found a release, wired to `install_app_update`), and a right cluster of the sync freshness pill, the always-on-top toggle, the settings key, and the close key. The bar and its non-interactive children carry `data-tauri-drag-region`, so a decorationless window stays draggable. The sync pill is a `role="status"` `aria-live="polite"` region showing real elapsed time since the last successful read; it absorbs the old usage pill vocabulary as slate variants (`offline` beats `cached` beats `paused`, mirroring the precedence those pills used) and never turns red. The always-on-top toggle reads and writes the persisted `always_on_top` setting through [[src/hooks/useRuntimeSettings.ts#useRuntimeSettings]], so the tray checkitem, the Settings toggle, and the titlebar are one state; a failed write surfaces as a toast rather than a button that lies. Both the settings key and the ⌘M/Ctrl+M accelerator registered in [[src/main.tsx]] go through [[src/lib/manageWindow.ts#openManageWindow]], which focuses an existing Manage window instead of stacking a second one.

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

### Widget Viz Kit

Four SVG primitives under `src/components/widget/viz/` draw every widget chart with no charting dependency, so the Flat Polish treatments (surface-faded overlay, hover-only legend, endpoint markers) stay under direct control.

Recharts is hostile to those treatments and costs 3.7 MB for shapes the widget can emit in ~300 lines, so the kit owns all widget visualization. The primitives take plain value arrays and a colour, never a data-frame; scaling and curve construction live in one shared module.

- **Sparkline** — [[src/components/widget/viz/Sparkline.tsx#Sparkline]] renders the 13px trend line under each readout cell: metric-hue stroke at 60% opacity plus a solid endpoint dot, no axes or ticks. Without a `label` prop it is `aria-hidden`, because the adjacent readout already states the number.
- **AreaChart** — [[src/components/widget/viz/AreaChart.tsx#AreaChart]] stacks one smoothed area per provider on a shared scale, keeps the series inside the lower ~62% so the overlaid headline never collides, and exposes an `overlay` slot plus a legend chip that is hidden at rest and revealed on hover or focus. Gradient ids are derived from `useId()` with punctuation stripped so `url(#…)` stays valid. A range with no values renders the shared empty state at the chart's own height, so nothing below shifts.
- **Bars** — [[src/components/widget/viz/Bars.tsx#Bars]] renders horizontal magnitude rows on a shared scale; each track is a real `role="progressbar"` with `aria-valuenow`/`min`/`max` and a formatted `aria-valuetext`, so the value is announced rather than inferred from pixel width.
- **Heat** — [[src/components/widget/viz/Heat.tsx#Heat]] renders a coarse density strip: one cell per bucket, magnitude carried by opacity of a single hue with a floor so an empty bucket still reads as a bucket.
- **geometry** — [[src/components/widget/viz/geometry.ts#scalePoints]] maps values into viewBox coordinates and [[src/components/widget/viz/geometry.ts#smoothPath]] builds the catmull-rom-to-bezier curve used by `specs/018-widget-ui-redesign/mockup.tpl.html`, so shipped charts trace the mockup's silhouette. Every helper returns new arrays or strings and never mutates its input.

### Widget Limits Band

[[src/components/widget/LimitsSection.tsx#LimitsSection]] is the widget's whole subscription readout: one row per enabled provider, and no section at all when none is enabled.

Each row is an identity swatch (the fixed provider hue), the provider name, one fixed-width cell per rate-limit window, and a right-aligned countdown to that row's nearest upcoming reset. The cells hold their column width so rows scan as a table at 360px whatever their bucket count, shrinking only when a provider reports more windows than the row can seat. A cell states the rounded percent, a window label compressed from the bucket label (the untouched label stays in the cell title and the bar's accessible name), and a 4px `role="progressbar"` bar carrying `aria-valuenow`/`min`/`max`.

Severity is carried on `[data-severity]` and follows the same 50/80 thresholds as [[lat.md/features#Features#Live Usage View]]: amber from 50%, red from 80%. A bucket whose `resets_at` has already elapsed is marked stale instead, which matches no severity rule and therefore renders neutral — a utilization measured against a bygone window must never read as a live severity. For the same reason an elapsed window is not a candidate for the row's nearest-reset countdown; the countdown falls back to "now" only when every dated window in the row has rolled over.

A provider with no live buckets still gets a row, stating why in the app's existing pill wording but in the widget's flat dress (a lamp and a word, no box): `SETUP` in amber when the failure is actionable — a `config`/`auth` provider error, or an unfinished install — and `UNAVAILABLE` in slate otherwise, since a degraded read is never an alarm. Before the first usage poll lands the row shows skeleton cells of the same geometry, so the real numbers do not move the rows beneath them. MiniMax rows keep the plan-level bucket filter (M\*, coding-plan-search, coding-plan-vlm) that [[src/components/live/ProviderUsageModule.tsx]] applies, because the per-model long tail does not fit a 360px row.

### Widget View Region

[[src/components/widget/ViewRegion.tsx#ViewRegion]] owns everything below LIMITS: one band header carrying the view name and the shared range strip, then whichever view that name selects.

View and range both live in the region rather than inside a view, so switching views keeps the operator's range and the mockup's single control strip stays single. A compact view is registered by adding one entry to the region's `VIEWS` list; only registered views reach the dropdown, so the list can never offer a view that would render nothing. The range vocabulary is 1H/6H/24H/7D — `30d` is deliberately absent, because a month is not a widget scope — and defaults to 6H, the shortest window that still spans a working session.

[[src/components/widget/ViewSwitcher.tsx#ViewSwitcher]] is a listbox rather than a menu, because the control has a value: the trigger is `aria-haspopup="listbox"` with `aria-expanded`, the popup carries exactly one `aria-selected` option, and keyboard movement runs through `aria-activedescendant` so focus never leaves the list. Escape, Tab and an outside click all close it, and Escape returns focus to the trigger.

#### Usage View

[[src/components/widget/views/UsageView.tsx#UsageView]] is the widget's default view and the product's core surface: hero chart, insight line, a 3×2 readout grid, the switchable breakdown, and the totals footer.

Every band reads the region's selected range, so chart, delta, insight, all six readouts, all six sparklines and the footer always describe the same window — a band quietly using a different window would be a lie about the instrument. The headline overlaid on the chart is `total_tokens` from [[src/hooks/useWidgetSeries.ts#useProviderTokenSeries]], which is the same figure the plotted areas sum to by construction. Its delta is momentum *inside* the range (the back half of the buckets against the front half) rather than a comparison with the previous window: a headline delta whose evidence the chart does not draw is not evidence.

Colour carries meaning and nothing else. Each readout's fixed metric hue appears only on its label swatch, its sparkline stroke and that stroke's endpoint; values stay `--text-hi`. Green and red on a delta are assigned by *meaning*, never by arrow direction — `InsightTrend.upIsGood` decides, so a falling tokens-per-LOC reads as the improvement it is, and a trend whose goodness is unknown stays neutral.

Per-metric sources: runtime and its sparkline from [[src/hooks/useLlmRuntimeStats.ts#useLlmRuntimeStats]]; tokens-per-LOC and LOC-per-hour with their trends from [[src/hooks/useCodeInsights.ts#useCodeInsights]]; sessions and projects sparklines from [[src/hooks/useWidgetSeries.ts#useActivitySeries]]; net lines from [[src/hooks/useCodeStats.ts#useCodeStats]], bucketed as `lines_added − lines_removed`. The footer's In/Out/Cache totals come from a range-scoped `get_token_stats` read rather than [[src/hooks/useTokenData.ts#useTokenData]], because a background instrument should not pay for the point history and hostname list it never draws.

The insight line carries one computed insight per window, chosen by the rotation rule in [[src/components/widget/views/insightLine.ts#selectInsightLine]] rather than pinned to a single source. Every candidate restates figures the view already read for that same window: context savings from [[src/hooks/useContextSavingsStats.ts#useContextSavingsStats]], the cached-token volume behind the footer's `Cache` percentage, and the per-provider split behind the hero chart. A candidate speaks only when its figure exists and is non-zero, so nothing is ever zeroed or padded to keep the line occupied; with no eligible candidate the line is simply not drawn.

Priority is fixed and ordered by how much of the story the rest of the widget does not already tell — savings appears nowhere else, the cached-token volume is only implied by the footer, and the provider split is already drawn directly above, so it speaks last. The first eligible candidate wins, and a higher-priority candidate whose source has not answered yet holds the line empty rather than letting a lower one appear and be swapped out a moment later; a failed read counts as answered-with-nothing, so one broken source cannot mute the line. Selection is a pure function of the window and its resolved data — no clock and no counter rotates it under the reader, so the same window with the same data always states the same thing.

The breakdown switches five modes over one row grammar — status dot, name, identity chip, dim secondary count, primary value, recency — filled per mode: Sessions (provider chip, tokens, live count in the header), Projects (session count, tokens), Hosts (turns, tokens), Skills (uses, last used), Hooks (QUILL chip where Quill-deployed, fires, last fired). Honesty disclosures keep the home that matches their data: the Hooks header carries the Claude/Codex tracking-asymmetry help, and the condensed retention line sits in Sessions, the only mode whose source [[lat.md/frontend#Frontend#Components#Retention Degradation]] actually prunes — skill and hook counts are never pruned, so claiming loss there would be its own lie.

#### Models View

[[src/components/widget/views/ModelsView.tsx#ModelsView]] answers "what am I running, and what did the work" in two bands: a running-now strip, then the session-ranked model list.

Both bands read the region's range through [[src/hooks/useModelAnalytics.ts#useModelAnalytics]], so one usage-overview snapshot serves the whole view and the hook's own coalescing — a one-second window after a committed model event, a 60-second fallback poll — keeps a background instrument off the backend's back. There is no inspect panel: session paging and chain history stay with the full page, so a widget row is deliberately inert.

Identity obeys DESIGN.md's Model-Shade Rule exactly as the full page does. Each model renders as a rank-assigned shade of its provider's family ramp (Claude orange, Codex blue, every other provider violet, rank seven and beyond neutral), assigned once per response from the delivered session-ranked order so both bands agree on a model's shade. A swatch never stands alone — it rides beside the raw id, qualified by a provider chip, and an unrecognized provider keeps a neutral chip rather than borrowing another family's hue. Ids are mono, rendered exactly as observed and ellipsized when they outgrow the column with the full string in `title`; no catalog, alias, or friendly name participates. The ranked list shows the top five models on one shared session scale, each track a real `role="progressbar"`, with attributed tokens beside the id.

Two disclosures keep a compact home here. Coverage states the share of token activity that carries model evidence whenever it is short of 100%, because activity recorded before a chain's first observation stays unattributed instead of being assigned a model. A retained-history line appears only while the backfill needs attention, carrying its state, its processed-source count, and a Retry while the run is retryable; a refused retry states its reason rather than vanishing. Emptiness is a claim the view has to earn: it names the specific negative — no retained sessions, no sessions in range, or sessions carrying no model identifier — only when the backend calls the scope final and the history inventory complete, and otherwise says the evidence is still being processed.

### Analytics Components

Analytics components in `src/components/analytics/` provide Now, Trends, Charts, Models, and an optional Context tab.

- **NowTab** (214 lines) — Real-time metrics with range selector (1h/6h/24h/7d/30d), six insight cards, a 24-hour activity heatmap, and a switchable breakdown panel (sessions/projects/hosts/skills).
- `NowTab` shares one comparison-range code-history fetch between the efficiency and velocity cards via `src/hooks/useCodeInsights.ts`, which avoids firing the same `get_code_stats_history` IPC call twice per refresh. The same hook fetches `get_llm_runtime_stats` so velocity divides LOC by active LLM runtime (matching the LLM Runtime card) instead of the wall-clock span; the prior window's active seconds are recovered by prorating the comparison-range runtime sparkline, and both periods fall back to wall-clock when no runtime is recorded.
- Selecting a session in `NowTab` now keeps provider identity alongside `session_id`, so token charts, compact token stats, and delete actions stay scoped to the correct Claude or Codex session.
- **TrendsTab** (105 lines) — Token trends, code velocity, and cache efficiency charts with week-over-week comparison.
- **ChartsTab** (454 lines) — Composite Recharts chart with three axes (utilization, tokens, LOC). Lazy-loaded with Suspense.
- **TabBar** — Analytics' horizontally scrollable underline navigation keeps Models available alongside Now, Trends, Charts, and optional Context. It uses stable tab/panel IDs plus roving Arrow/Home/End keyboard focus and activation.
- **ModelBackfillStatus** — [[src/components/analytics/models/ModelBackfillStatus.tsx#ModelBackfillStatus]] is a one-line annunciator rendered only while backfill is pending, running, partial, or failed, or when a retry request errors; a clean complete pass keeps only its visually hidden live announcement. The state word maps to the reserved severity colors, partial/failed expose a ghost Retry, and the aria-live region announces state changes atomically without hiding recovered data.
- **ModelRunningNow** — [[src/components/analytics/models/ModelRunningNow.tsx#ModelRunningNow]] shows each provider's current model: the latest contiguous run with when it took over and the model it replaced, colored from the shared shade map.
- **ModelUsageSpine** — [[src/components/analytics/models/ModelUsageSpine.tsx#ModelUsageSpine]] ranks models by distinct sessions into selectable `aria-pressed` rows: a sessions bar scaled to the top model plus projects, primary-in, turns, days active, and tokens columns.
- **ModelActivityChart** — [[src/components/analytics/models/ModelActivityChart.tsx#ModelActivityChart]] renders Recharts stacked bars of distinct sessions per model per fixed bucket, grouping stacks by provider with the top four models as named shaded series and the remainder folded into one neutral `other` series, mirrored by a visually hidden semantic table.
- **ModelProjectMatrix** — [[src/components/analytics/models/ModelProjectMatrix.tsx#ModelProjectMatrix]] crosses the overview's top projects against models with per-cell session counts in shade-tinted cells.
- **ModelCombinations** — [[src/components/analytics/models/ModelCombinations.tsx#ModelCombinations]] charts the models-per-session distribution as bars plus the top co-occurring model pairs.
- **ModelDelegation** — [[src/components/analytics/models/ModelDelegation.tsx#ModelDelegation]] splits attributed tokens between parent sessions and subagent chains as a proportion meter with each group's top model.
- **ContextSavingsTab** — Context preservation analytics with a four-column stats strip (saved, indexed, returned, routing) over a stacked trend chart, breakdown table, and recent events feed. Breakdown rows render a relative-magnitude bar fill behind each row scaled to the largest event count, and recent events use a single-line log format with category swatches and a directional byte arrow (→ indexed, ← returned). Confidence is hidden for exact estimates. `AnalyticsView` shows this tab when context preservation is enabled or historical context-savings events exist; a persisted active Context tab remains mounted while that status is unresolved and resets only after a successful status read proves it unavailable.
- **UsageChart** (456 lines) — `ComposedChart` with Area, Line, and custom Tooltip. Uses `ChartCrosshairContext` for tooltip synchronization.
- **BreakdownPanel** — Sortable table showing sessions, projects, hosts, or skills with compact count columns. It renders all rows in a flexing scroll area that fills the available analytics pane height instead of paginating the breakdown. Session rows display provider badges and use provider-safe composite keys for selection. Hosts and projects show `<recency>` in their time column (e.g. `2h ago`); sessions show `<recency> · <duration>` (e.g. `23h ago · 23h 43m`, or `active · 6m` when `last_active` is within the last 5 minutes), so the SQL `last_active DESC` ordering is visible without hiding session length. Skills rows show recognized use count and `last_used` recency — provider breakdown lives in the filter strip rather than inline on each row, so the count column stays uncluttered; their controls render on a dedicated row directly beneath the breakdown mode tabs and intentionally use a different visual vocabulary than the chunky `.range-tab` container pills above: an underline-indicator text filter strip (`All / Codex / Claude`) sits left-aligned, and a right-justified outlined uppercase `∞ ALL TIME` chip toggles the all-history scope. A Skills-only header row labels Skill, Uses, and Last used as small sort buttons; the default is Uses descending, and clicking the active title flips direction without refetching from Tauri. The three shape languages (container pills, underline filters, outlined glyph chip) keep each control reading as its own thing instead of three stacked rows of identical buttons, and the Skills-specific filters never crowd the mode tabs or affect the Now range selector. Every skill row renders the shared tiny hairline disclosure caret and lazy-fetches per-(project, hostname) counts via [[src/hooks/useSkillProjects.ts#useSkillProjects]] when opened, including rows whose `project_count` is `1`; the drilldown renders indented sub-rows below the parent skill and labels null-project rows as `No project data` so child counts still sum to the parent. Sub-rows reuse the sub-agent tree-guide and indent CSS for visual consistency with session→sub-agent drilldowns and carry a dedicated `breakdown-row-skill-project` class for future styling overrides. Switching filter scope (provider/all-time) collapses every expanded skill so stale sub-rows cannot survive a filter change. Per-mode SQL caps bound the payload: hosts 50, projects 100 (pre-subdir-merge), sessions 200 (passed from `useBreakdownData`'s `SESSION_BREAKDOWN_LIMIT`), and skills 100 (from `SKILL_BREAKDOWN_LIMIT`). For sessions whose rollup reports `has_subagents = true`, [[src/components/analytics/BreakdownPanel.tsx#SessionTreeBranch]] manages the per-row expand state and renders the lazy-fetched sub-agent tree through [[src/components/analytics/BreakdownPanel.tsx#SubagentRow]] — a recursive renderer depth-bounded by `SUBAGENT_MAX_DEPTH = 10` that uses [[src/hooks/useSessionSubagents.ts#useSessionSubagents]] for caching; non-expandable session rows omit the disclosure slot so their ids stay flush with normal row padding. In `sessions` mode only, the panel renders the retention banner and marks every row whose span falls at or before the retention cutoff — see [[frontend#Frontend#Components#Retention Degradation]].
- **Insight cards**: `InsightCard` (generic), `SessionHealthCard`, `ProjectFocusCard`, `LearningProgressCard` — each shows a metric with trend arrow and sparkline. `InsightCard` also accepts an optional `description` prop that renders a top-right `?` help button and a sibling `.insight-card-tooltip` span; the [[features#Analytics Dashboard#Now Tab]] right-column context-savings cards opt into this for in-place metric explanations.
- **Sparklines**: `TokenSparkline`, `CodeSparkline`, `MiniChart` — small inline Recharts charts.
- **Utility**: `TabBar`, `TogglePills` (range selector), `ActivityHeatmap`, `CompactStatsRow`, `shared.tsx` (getColor, TrendArrow).

### Models Composition

`ModelsTab` coordinates range, provider, and provider-qualified model selection without treating raw model identifiers as product configuration.

The user-facing metrics and investigation contract is summarized in [[features#Analytics Dashboard#Models Tab]].

[[src/components/analytics/ModelsTab.tsx#ModelsTab]] orchestrates the page: range and provider `aria-pressed` control groups with a `N sessions · N projects` caption, the conditional backfill annunciator, scope notices, then sections in fixed order — [[src/components/analytics/models/ModelRunningNow.tsx#ModelRunningNow]], [[src/components/analytics/models/ModelUsageSpine.tsx#ModelUsageSpine]], [[src/components/analytics/models/ModelActivityChart.tsx#ModelActivityChart]], [[src/components/analytics/models/ModelProjectMatrix.tsx#ModelProjectMatrix]], [[src/components/analytics/models/ModelCombinations.tsx#ModelCombinations]], [[src/components/analytics/models/ModelDelegation.tsx#ModelDelegation]], and an Inspect section that docks [[src/components/analytics/models/ModelDetailPanel.tsx#ModelDetailPanel]] when a spine row is selected — closed by a footer caption restating attributed-of-total tokens, model count, and generation time. Provider labels, badges, and relative-time formatting live in the shared [[src/components/analytics/models/modelFormat.tsx]] helper, which also owns the provider-family model shade system: `CLAUDE_SHADES` (orange family) and `CODEX_SHADES` (blue family) assigned by in-scope rank through [[src/components/analytics/models/modelFormat.tsx#buildModelShadeMap]], one map per overview so a model keeps the same shade in every section. Token magnitudes render through [[src/utils/tokens.ts#formatTokenCount]], which carries a billions tier used app-wide.

[[src/hooks/useModelAnalytics.ts#useModelAnalytics]] now fetches one [[src-tauri/src/lib.rs#get_model_usage_overview]] snapshot per scope; its event coalescing, refresh-generation, and backfill retry machinery are unchanged. Provider controls derive from overview responses, a failed refresh keeps the last loaded overview visible beside a request-local Retry, and layout-time reconciliation still clears provider or model selection before paint when no longer represented. Model selection is client-side and never changes the overview request identity; the Inspect drill-down keeps [[src-tauri/src/lib.rs#get_model_sessions]] paging and [[src-tauri/src/lib.rs#get_session_model_history]] chain history.

The retained-history status is independent from overview request errors. A final empty claim requires persisted inventory completeness, clean complete backfill counters, and backend `scopeFinal`; it then applies global-session, filtered-session, and reliable-evidence precedence. Pending, running, partial, failed, or retrying history instead labels the scope provisional or incomplete while leaving recovered overview data mounted. Backend scope facts already exclude suppressed sources, so the frontend never reconstructs emptiness from visible rows.

Selecting a model mounts the session panel with both detail hooks consuming the same frontend refresh generation. Paging and expanded histories refresh independently. After a bounded `not_found` notice, composition hides only that exact provider/session row for the active range and model identity. Old-scope callbacks are ignored; a successful page snapshot that omits the row clears the local hide marker so later valid reappearance remains possible.

[[src/components/analytics/AnalyticsView.tsx#AnalyticsView]] owns the Models range independently from snapshot-backed ranges and restores Models from persisted tab state. Every visible tab keeps a lightweight panel shell using the stable IDs exported by [[src/components/analytics/TabBar.tsx#analyticsTabId]] and [[src/components/analytics/TabBar.tsx#analyticsPanelId]]. Models content is lazy until first visit, then remains mounted under that shell across internal Analytics tab switches; the hidden panel preserves its request state, fixed-window event listener, fallback poll, filters, and selection without preloading Models or repeating listener-gap reconciliation on each opening. Other data-heavy content mounts only for the effective active panel. Definitively unavailable Context state maps immediately to Now before the persistence effect runs. Snapshot polling mounts only with active Now, Trends, or Charts content; [[src/hooks/useAnalyticsData.ts#useAnalyticsData]] marks snapshot-count readiness only after a successful count response, so the shared empty state cannot mistake the default zero during failure recovery for confirmed absence. Models and Context remain independent from snapshot requests and failures.

### Model Session Detail Panel

`ModelDetailPanel` presents selected-model session paging and lazy chain history while keeping asynchronous state in its hooks.

[[src/components/analytics/models/ModelDetailPanel.tsx#ModelDetailPanel]] owns only the open disclosure keys. Native buttons expose stable controlled panel IDs; page replay, pagination, row refresh, retained errors, and stale-session notices remain independently visible. Each expanded session shows provider-qualified identities, range-scoped totals, parent/subagent metadata, and backend-ordered model or identity-gap segments without interpreting raw identifiers.

The panel header pairs a deselect control with a token-split description list (input, output, cache-write, first seen) for the selected model. Session rows are hairline-divided with neutral model/chain/switch count chips, and each expanded chain draws a proportional timeline strip — subagent chains labeled in Agent Orchid, selected-model segments signal-blue — above the chronological segment list.

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
- **ResultCard** — Search hit preview with provider badge, snippet, and per-session code-change pill. Takes the retention cutoff and swaps the line counts for a pruned marker when the hit predates it — see [[frontend#Frontend#Components#Retention Degradation]].
- **DetailPanel** — Context message display with provider badge, match highlighting, and session-local code-change totals, with the same pruned marker as `ResultCard`.

### Retention Control

The Performance tab's prune control (feature 014): a preset selector, a preview step, an explicit second click, and a durable record of the last run — the only place in the product that can delete history.

[[src/components/settings/PerformanceTab.tsx#PerformanceTab]] drives it from one
`RetentionStage` union (`idle` → `previewing` → `confirm` → `running` → `done`,
plus `declined`), so the panel below the row is a function of a single value
rather than of four booleans that can contradict each other. Confirming
**re-previews** before invoking `run_retention_maintenance`: the backend refuses
a confirmation that no longer matches a freshly counted cutoff, so the token the
user consents to is minted at the moment of consent instead of being held open
while the panel sat on screen. Two backend reasons are matched exactly rather
than rendered as prose — `stale_preview` and the lease refusal — because their
remedy is an action (`Count again`), not copy.

[[src/components/settings/PerformanceTab.tsx#RetentionPanel]] is the consent
step and every terminal state. It names the capability loss from the preview's
`affected_surfaces`, flags the case where the cutoff covers *every* transcript
row, and offers `Archive & prune` beside `Prune without archive`. The archive
choice includes the preview-reported non-conforming rows even though SQLite
keeps them, and the terminal state reports its full local JSONL path and counts.
The panel repeats [[src/components/settings/PerformanceTab.tsx#RECLAIM_SENTENCE]]
wherever the rows/bytes distinction could mislead. `partial` gets its own
heading and states that what was removed is gone permanently.

[[src/components/settings/PerformanceTab.tsx#RetentionAudit]] renders the
durable `retention.last_run` record so the outcome survives the toast, and
[[src/components/settings/PerformanceTab.tsx#retentionAgeLine]] is the only
mitigation the no-scheduler decision gets: a window is a plan, not a timer, so
the record states its own age beside the configured window ("last pruned 112
days ago; window 90 days"). A `skipped` record says *attempted*, never *pruned*.

A single `maintenanceBusy` expression disables both Compact and Prune, because
both take the same process-wide ingest quiesce and the backend's non-blocking
acquire turns a double-click into a skip the user would have to read rather than
a button that could not be pressed. Styling stays chrome-grey and hairline-ruled
throughout: `DESIGN.md` reserves green / amber / red for the severity meter, and
a prune the user asked for is not a threshold breach — the weight of the action
is carried by the copy and by the second click, never by a red button.

### Retention Degradation

The consumer-side treatment for retention pruning (feature 014): the surfaces whose data retention can delete state the cutoff and mark pre-cutoff figures, so deleted history never renders as an honest zero.

[[src/hooks/useRetentionCutoff.ts#useRetentionCutoff]] is the read-only cutoff
reader mounted by those surfaces. It reads [[backend#Backend#Tauri IPC Commands#Retention policy commands|get_retention_policy]] and exposes the **watermark**, not the
configured window: the window is a standing intention, the watermark is the
durable fact about what was actually removed, and only the fact may be shown to
a user as a date. It re-reads on `retention-maintenance-finished` so a banner
that appears mid-session states the new cutoff, and a failed read leaves the
cutoff null — degrading to the pre-014 rendering rather than to a banner
asserting a boundary that may not exist. It is deliberately separate from the
Settings control's policy hook, which can also write.

[[src/components/RetentionBanner.tsx#RetentionBanner]] renders the cutoff plus a
per-surface footnote and returns nothing when the cutoff is null. Its
`RetentionSurface` union is the scope of the whole treatment, and it is small on
purpose: `range_to_duration` caps every range-based reader at 30 days and the
retention preset floor is 30 days, so `get_code_stats`,
`get_code_stats_history` and `get_llm_runtime_stats` provably cannot reach a
pruned row and must **not** carry the banner — claiming loss where there is none
is as dishonest as hiding loss where there is. Only the session-scoped readers
degrade: `get_session_breakdown` and `get_session_subagent_tree` (surface
`sessions`, in [[src/components/analytics/BreakdownPanel.tsx#BreakdownPanel]])
and `get_batch_session_code_stats` (surface `session-search`, in
[[src/windows/SessionsWindowView.tsx]]). Styling is chrome-grey by design:
DESIGN.md reserves green/amber/red for the severity meter, and a boundary the
user opted into is a fact about the instrument, not an alarm.

[[src/utils/retention.ts]] holds the pure helpers.
[[src/utils/retention.ts#retentionSpanFor]] classifies a `[first_seen,
last_active]` span as `retained` / `straddles` / `pruned`;
[[src/utils/retention.ts#markPrunedRange]] applies it across a whole
time-ordered range, returning new `{ row, span }` pairs rather than mutating the
rows; [[src/utils/retention.ts#isPruned]] is the single-instant form; and
[[src/utils/retention.ts#PRUNED_PLACEHOLDER]] is the em dash that replaces a
zero which is really absent data. Two conservatisms are deliberate, both erring
towards *not* marking: an unparseable timestamp reports as retained (mirroring
the delete engine's `length(timestamp) = 24 AND timestamp LIKE '%Z'` conformance
guard, which refuses to delete rows it cannot compare), and "pruned" means
*pre-cutoff*, never *provably empty* — live rows and non-conforming timestamps
survive below the watermark, so all copy says "may be incomplete".

#### Mixed-Horizon Sub-Agent Counts

`SessionBreakdown.subagent_count` is an accepted, documented limitation rather than a bug to fix here: it can outlive the tree it summarises, so the Sessions breakdown renders it marked instead of exact.

The count unions `token_snapshots ∪ response_times ∪ tool_actions` and retention
prunes only the last of the three, so for a session older than the watermark it
is computed over mixed horizons and can disagree with its own drilldown — the
badge says `+2`, the expanded tree says nothing. The treatment is to dagger the
badge, explain the mixed horizon in its title, and replace the tree's "No
sub-agents" empty state with "Sub-agent detail pruned (before <date>)" so the
contradiction is named rather than left for the user to discover. `has_subagents`
degrades the same way. The real fix is rollup aggregates, which are a deferred
follow-up.

The same shape appears in session search from the other direction: the
full-text index is never pruned, so a hit survives after the SQL rows behind its
code stats are gone. That is why the `session-search` footnote says search
itself is unaffected — the result is real, only its drilldown is empty.

#### All-Range Retention Invariant

A forward-looking rule recorded on `RangeType` in [[src/types.ts]] rather than shipped as an edit, because the edit S4 originally asked for would be a lie against today's code.

S4 asked for any `all` range to be relabelled "all retained". Grounded against
the code that is vacuous: `RangeType` is `"1h" | "24h" | "7d" | "30d"` with no
`all` member, and `range_to_duration` has no `all` arm to feed one. The only
"All time" affordances in the product are the two Breakdown toggles, and they
read `skill_usages` and `hook_invocations` — tables retention never prunes — so
relabelling *them* would itself be false. The requirement therefore survives as
an invariant instead of an edit:

> Any future all-time or otherwise unbounded range added to `RangeType` that
> reads `tool_actions` or `session_events` must be labelled "all retained"
> rather than "all time", and must render `RetentionBanner` on every surface
> that draws it.

The 30-day cap is what makes the three range-based readers provably unaffected,
so an unbounded range is precisely the change that breaks the proof.

### Restart Component

Controls for restarting Claude Code instances from the dedicated Restart window.

- **RestartPanel** (`src/components/restart/RestartPanel.tsx`, 205 lines) — Instance list with status indicators, force restart option, and hook installation prompt.

## Custom Hooks

All data hooks use Tauri `invoke()` for request-response and `listen()` for push event refresh. Most refresh on a 60-second interval and debounce event-triggered refreshes by 1 second.

### Integration Hook

`useIntegrations` in [[src/hooks/useIntegrations.ts]] loads provider statuses plus the persisted indicator primary provider, listens for `integrations-updated` and `indicator-updated`, and tracks per-provider in-flight actions.

It drives the [[features#Settings Window]]'s Integrations tab and blocked-window gating. The `enableProvider` function accepts an optional `apiKey` argument used by service-only providers like MiniMax, while `saveIndicatorPrimaryProvider` persists the status-indicator preference without introducing a separate frontend polling path. `rescan` invokes the `rescan_integrations` IPC and tracks `rescanInFlight` so the "Rescan PATH" row can spin while the backend re-derives the login-shell PATH and re-runs detection.

### Settings Hooks

Five hooks back the [[features#Settings Window]]: each owns one slice of state, calls Tauri IPC for mutations, and subscribes to the matching push event so multiple open Settings windows stay in sync.

| Hook | File | Source of truth | Listens for |
|------|------|-----------------|-------------|
| `useIntegrationFeatures` | [[src/hooks/useIntegrationFeatures.ts]] | `IntegrationFeatures` global flags (context preservation, activity tracking, context telemetry) | `integration-features-updated` |
| `useRuntimeSettings` | [[src/hooks/useRuntimeSettings.ts]] | `RuntimeSettings` background-task tunings (live-usage interval, rule watcher, always-on-top) | `runtime-settings-updated` |
| `useLearningSettings` | [[src/hooks/useLearningSettings.ts]] | `LearningSettings` (trigger mode, periodic interval, thresholds) | None — read on mount and after save |
| `useUiPrefs` | [[src/hooks/useUiPrefs.ts]] | `UiPrefs` localStorage values (layout mode, time mode, panel visibility) | `ui-prefs-updated` (frontend-emitted across windows) |
| `useRetentionPolicy` | [[src/hooks/useRetentionPolicy.ts]] | `RetentionPolicy` (window, watermark, last run) via `get_retention_policy` / `set_retention_policy` | `retention-maintenance-finished` |

`useIntegrationFeatures` exposes typed setters per flag that each invoke a dedicated `set_*_enabled` IPC, while `useRuntimeSettings` and `useLearningSettings` save the whole struct in one call.

[[src/hooks/useRetentionPolicy.ts#useRetentionPolicy]] is deliberately **not**
part of `RuntimeSettings`. `PerformanceTab.update()` saves that struct wholesale
(`{ ...settings, ...patch }`), which is right for a set of independent
background-task tunings and wrong for a destructive boundary: a retention window
is consented to one value at a time, and a wholesale save would let an unrelated
toggle re-assert a window the user never looked at. So the policy travels on its
own commands, and
[[src/hooks/useRetentionPolicy.ts#RETENTION_WINDOW_PRESETS]] mirrors the backend
preset set rather than offering a free-form number the command boundary would
reject. `setWindowDays` stores the policy the backend *returns* instead of an
optimistic one, so a rejected preset leaves the control holding the window the
database actually has. It re-reads on `retention-maintenance-finished` because a
completed run advances the watermark and rewrites the audit record. It is the
only retention hook that can write; the read-only counterpart is
[[src/hooks/useRetentionCutoff.ts#useRetentionCutoff]], described in
[[frontend#Frontend#Components#Retention Degradation]]. `useUiPrefs.update(patch)` writes localStorage and emits `ui-prefs-updated` so the main window's [[src/App.tsx]] re-applies layout / time-mode / panel-visibility without a reload.

### Data Fetching Hooks

Hooks that invoke Tauri commands and return async state (data, loading, error).

| Hook | Returns | Tauri Commands |
|------|---------|----------------|
| `useAnalyticsData` | Range-scoped usage history and stats; receives the parent-owned snapshot state for empty-state consumers | `get_usage_history`, `get_usage_stats` |
| `useLiveSummaryData` | Aggregate live `Sessions`, `Projects`, and range-scoped `Tokens` cards across enabled providers | `get_session_breakdown`, `get_token_history` |
| `useTokenData` | Token history with hostname/session filtering; hostnames load independently of range changes | `get_token_history`, `get_token_stats`, `get_token_hostnames` |
| `useProviderTokenSeries` | Aligned per-provider token series for the widget hero chart, on the shared 8-bucket grid | `get_provider_token_series` |
| `useActivitySeries` | Per-bucket distinct session and project counts for the widget sparklines, on the same grid | `get_activity_series` |
| `useCodeStats` | Lines added/removed by language | `get_code_stats`, `get_code_stats_history` |
| `useBreakdownData` | Host/project/session breakdown tables | `get_host_breakdown`, `get_project_breakdown`, `get_session_breakdown` |
| `useSessionHealth` | Avg duration, tokens, sessions/day with trend | `get_session_stats` |
| `useActivityPattern` | 24-hour hourly token distribution | `get_token_history` (derived) |
| `useLlmRuntimeStats` | Cumulative runtime, session count, turn count, avg per turn, sparkline | `get_llm_runtime_stats` |
| `useEfficiencyStats` | Tokens-per-LOC ratio with trend | Derived from token + code stats |
| `useVelocityStats` | LOC per active LLM-runtime hour with trend | Derived from code stats + `get_llm_runtime_stats` |
| `useRetentionCutoff` | Read-only retention watermark + window for the degradation treatment; re-reads on `retention-maintenance-finished` | `get_retention_policy` |

| `useLearningStats` | Rule counts by state, confidence buckets | `get_learned_rules` (derived) |
| `useLearningData` | Rules, runs, settings, observations, logs | Multiple learning commands + events |
| `useMemoryData` | Memory files, suggestions, projects | Multiple memory optimizer commands |
| `useSessionCodeStats` | Batch LOC stats per session (ref-cached) | `get_batch_session_code_stats` |
| `useCacheEfficiency` | Cache hit rate (derived from token history) | None (derived) |
| `useContextSavingsStats` | Context savings summary, time series, breakdowns, and recent events; subscribes to `context-savings-updated`. Powers both the [[features#Analytics Dashboard#Context Tab]] strip and the right column of [[features#Analytics Dashboard#Now Tab]]. | `get_context_savings_analytics` |
| `useSessionSubagents` | Per-`(provider, session_id)` lazy sub-agent tree state for the Sessions breakdown's expandable rows; caches results so collapse/re-expand never refetches | `get_session_subagent_tree` |
| `useSkillProjects` | Per-`(skill_name, requestKey)` lazy project-breakdown state for the Skills breakdown's expandable rows; `requestKey` encodes `${mode}:${days}:${allTime}:${provider}` so cache slots invalidate on filter change while strictly lazy-fetching only on expand | `get_skill_project_breakdown` |
| `useModelSessions` | Selected provider-qualified model paging with atomic shared-refresh replay and operation-local recovery | `get_model_sessions` |
| `useSessionModelHistory` | Per-`(provider, sessionId, range)` lazy model-chain history for expanded Models rows; shared refresh refetches expanded rows, invalidates collapsed caches, and preserves good data behind row-local errors | `get_session_model_history` |

[[src/hooks/useCachedInvoke.ts#useCachedInvoke]] is the shared cache primitive
for `useModelAnalytics`, `useTokenData`, `useCodeStats`, `useCodeInsights`,
`useLlmRuntimeStats`, `useContextSavingsStats`, `useSessionHealth`,
`useActivityPattern`, `useBreakdownData`, and `useAnalyticsData`. Each hook
keeps identity-scoped accepted data, starts its first request immediately, and
debounces later refreshes by 200 ms. A generation guard discards stale
responses, a same-identity in-flight request coalesces refreshes, and equal
JSON results retain their prior object identity. This gives every ported hook
stale-while-revalidate rendering without duplicating request-lifecycle code.

`AnalyticsView` owns the single snapshot-count request for every empty-state
gate and passes it to the Now tab, while its runtime hook supplies
`useCodeInsights` so the comparison cards do not duplicate the current-window
runtime request. `useTokenData` also keeps range-independent hostnames in a
separate cache identity, avoiding a hostname IPC call on range changes.

`useLiveSummaryData` fetches provider-filtered token and session history on demand so the top workload rail can aggregate `Sessions`, `Projects`, and range-scoped `Tokens` across whichever providers are enabled, while the grouped row sections continue to consume the already-fetched `UsageData` snapshot from `fetch_usage_data`.

The analytics hooks for the `Now` tab subscribe to backend push events instead of relying only on the 60-second polling fallback. `useLlmRuntimeStats` and `useBreakdownData` refresh on `sessions-index-updated`; `useCodeStats` and `useCodeInsights` also subscribe to `transcript-analytics-updated`, because after migration 30 `tool_actions` is written exclusively by source-owned reconciliation and `sessions-index-updated` no longer covers it. `useCodeInsights` additionally listens to `tokens-updated` because it combines code and token history. The widget series hooks in [[src/hooks/useWidgetSeries.ts]] share one refresh path — both read `token_snapshots`, so both debounce on `tokens-updated` and poll on the same 60-second interval, and neither can end up a refresh behind the other.

`useMemoryData` tracks concurrent optimization runs by run id and uses background refreshes for event-driven updates so `Optimize All` does not drop out of the running state or flash the all-projects view on every completion event. The hook initializes the Memories tab to the aggregate `__all__` selection on first load, then reuses the project-scoped delete IPC command to support current-view bulk deletion in both single-project and all-projects modes.

### State Pattern

Hooks follow a consistent async state pattern: `useState` for data/loading/error, `useRef` for initial load tracking, `useEffect` for fetching, periodic interval refresh, and Tauri event listener cleanup.

### Model Analytics Hook

`useModelAnalytics` keeps usage-overview and backfill retry state independent so a failed refresh cannot replace the last successfully loaded same-scope overview.

[[src/hooks/useModelAnalytics.ts#useModelAnalytics]] takes `(range, provider, active)`, fetches the single `get_model_usage_overview` snapshot per scope, and exposes separate initial-loading, refresh-loading, structured-error, and Retry state. [[src/hooks/useCachedInvoke.ts#useCachedInvoke]] owns its identity-keyed cache, first-call-immediate/later-call-200ms debounce, stale-response generation guard, same-identity deferred refresh dedupe, and byte-identical reference reuse. Revisiting a range/provider scope therefore renders cached data while it revalidates without a skeleton. Backfill status persists across scope changes from accepted snapshots and the structured retry response; generation, lifecycle, inventory, and monotonic progress outrank wall-clock timestamps so clock rollback cannot hide completion. Its own guarded Retry never clears recovered overview data. External refreshes are gated: the `model-analytics-updated` listener ignores events whose payload reports `dataChanged === false`, and event-driven refresh and the 60-second poll pause while the panel is inactive or the document is hidden, replaying a single missed signal once on re-activation. One Strict Mode-safe listener still starts a fixed one-second deadline at the first observable event and reconciles once when its asynchronous registration becomes active to close the initial fetch/subscription gap unless a captured event already owns that refresh; a disposed registration only unsubscribes. Each accepted external refresh advances a frontend-only generation for mounted detail hooks; because that advance is now gated too, it transitively throttles the paged-session replay and per-row history refetch. Model selection is client-side and never refetches the overview.

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

Key type categories: usage/token tracking (`UsageBucket`, `TokenDataPoint`, `TokenStats`, `ProviderCredits`), context savings (`ContextSavingsAnalytics`, `ContextSavingsEvent`), indicator state (`IndicatorPrimaryProvider`, `IndicatorMetric`, `StatusIndicatorState`), analytics (`BucketStats`, `SessionHealthStats`, `ResponseTimeStats`), learning (`LearnedRule`, `LearningRun`, `LearningSettings`), session search (`SearchHit`, `SearchResults`, `SessionContext`), restart (`ClaudeInstance`, `RestartStatus`), memory (`MemoryFile`, `OptimizationSuggestion`).

Display enums: `TimeMode`, `RangeType`, `TrendType`, `BreakdownMode`, `SortMode`, `AnalyticsTab`. `RangeType` carries the all-range retention invariant in its doc comment, and `SessionBreakdown.subagent_count` / `SessionCodeStats` carry their retention degradation notes — both described in [[frontend#Frontend#Components#Retention Degradation]].

Retention types (`RetentionPolicy`, `RetentionPreview`, `RetentionAuditRecord`, `RetentionMaintenanceProgress`, `RetentionMaintenanceResult`) mirror [[src-tauri/src/retention.rs]] and keep snake_case because they arrive straight off `invoke()` with no mapping layer.

## Styling

Pure CSS with no framework, organized around a `:root` design-token layer in `src/styles/index.css` per DESIGN.md. Dark theme: near-black `--console-black` (`#121216`) canvas, `--readout` (`#d4d4d4`) text, 11px Geist with system fallback.

### Typography

Body/UI text is **Geist** and monospace contexts (ids, code, paths) are **Geist Mono** — both self-hosted variable fonts (weights 100–900) with system stacks as fallback.

Both are vendored from the `geist` npm package into `src/assets/fonts/` (`Geist-Variable.woff2`, `GeistMono-Variable.woff2`) and declared via `@font-face` in `index.css` with `font-display: swap`. Every window stylesheet's mono stack leads with `"Geist Mono"`.

### Design Tokens

The canonical palette lives as `:root` CSS custom properties in `src/styles/index.css`, following DESIGN.md. Because [[src/main.tsx]] loads `index.css` for every window, these tokens are global to all stylesheets.

Tokens cover backgrounds (`--console-black`, `--panel-deep`, `--panel-raised`, `--card-graphite`, `--slate-input`, `--graphite-line`), text (`--readout`, `--readout-bright`, `--label`, `--label-faint`), the status meter (`--meter-green` / `--meter-amber` / `--meter-red`), accents (`--signal-blue` / `--signal-cyan` / `--signal-violet` / `--signal-orchid`), provider identity (`--provider-claude` / `--provider-codex` / `--provider-minimax` / `--provider-agent`), and `--radius-*` / `--space-*` scales. Every window stylesheet reads its palette from these vars. The former Tokyo-night palette and divergent green and lifecycle colors, plus the GitHub-dark insight-card/tooltip sub-palette and assorted near-whites (`index.css`, `settings.css`) have all been unified onto the canonical tokens. The only remaining color literals are neutral white/black alpha — the dimming ladder — and one intentional lighter-green toggle-hover tint.

A second token block in the same `:root` carries the Flat Polish system for the 360px widget: the flat surface pair (`--surface` `#14181f`, `--inset`), hairline and hover alphas (`--line`, `--line-soft`, `--hover`), a brightness-only text ladder (`--text-hi`, `--text`, `--faint`), and six metric hues (`--metric-runtime`, `--metric-tok-per-loc`, `--metric-loc-per-hr`, `--metric-sessions`, `--metric-projects`, `--metric-net-lines`). The metric hues are permitted on sparkline strokes, endpoints, and label swatches only — values stay `--text-hi` and severity stays with the meter, so identity never reads as state. Provider hues are reused unchanged from the block above.

Alongside them, `index.css` defines the widget's shared primitives: `.wg-key` keycaps, `.wg-toggles`/`.wg-toggle` strips, the `.wg-pill` sync/status pill, `.wg-rule` hairline, `.wg-state` per-band empty/loading/error boxes, `.wg-skeleton` shapes, the `.wg-bar` utilization bar, and the `viz-*` classes the kit renders into. Accessibility is enforced through the selectors rather than left to callers: a toggle only renders as selected under `[aria-pressed="true"]`, the pill's degraded variants key off `data-state`, and the pulse and skeleton animations are wrapped in `prefers-reduced-motion: no-preference`.

### Stylesheets

Per-window CSS files under `src/styles/`, each scoped to a specific feature domain.

| File | Lines | Scope |
|------|-------|-------|
| `src/styles/index.css` | 3,798 | Global styles, main window, analytics, layout toggle |
| `src/styles/learning.css` | 940 | Learning window and components |
| `src/styles/sessions.css` | 498 | Session search window |
| `src/styles/restart.css` | 356 | Restart window |

### Color System

Semantic palette, drawn from the `:root` tokens. Status color is reserved; identity color is fixed per provider.

- **Status meter** (`--meter-green` `#34d399` < 50%, `--meter-amber` `#fbbf24` 50-80%, `--meter-red` `#f87171` >= 80%): utilization, trends, success/warning/error. Reserved for threshold state only.
- **Signal blue** (`--signal-blue` `#60a5fa`): accents, selection, focus rings, primary actions. The sessions search/filter focus and active-sort toggle use this — previously green, which collided with the meter.
- **Provider identity** — one fixed color per provider across all four surfaces (titlebar usage badges, breakdown tags, learning badges, session-search badges): Claude `--provider-claude` orange `#fb923c`, Codex `--provider-codex` blue `#60a5fa`, MiniMax `--provider-minimax` violet, sub-agent `--provider-agent` orchid. Blue/orange is the colorblind-safe two-group pairing, deliberately redder than caution amber so identity never reuses a status hue; the `shared` learning scope renders neutral. On the Models tab, individual models render as rank-assigned shades of their provider's family ramp.
- Memory type badges: blue (user), red (feedback), green (project), yellow (reference), purple (claude-md)
- Context savings categories: green (capture), blue (source), amber (router), purple (decision), pink (provider) — derived from the event-type prefix in [[src/components/analytics/ContextSavingsTab.tsx#categoryColor]] and reused by KPI swatches, breakdown dots, and event-line dots

### Responsive Scaling

The widget main window does not scale: it is fixed at 360px wide with a content-derived height, so the `--s` fit-to-height system and the per-layout `quill-size-*` sizes it depended on no longer have a consumer.

Accessibility zoom is unaffected — [[src/main.tsx]] still applies Ctrl+`+`/`-`/`0` webview zoom per window, and the widget's content column scrolls so a zoomed-in layout stays reachable. The remaining `--s` declarations in `src/styles/index.css` belong to components the widget redesign has not yet torn down.

## Utilities

Shared formatting and chart helper functions under `src/utils/`.

| File | Exports |
|------|---------|
| `src/utils/format.ts` | `formatNumber()` (thousand separators), `formatDurationSecs()` (human-readable) |
| `src/utils/tokens.ts` | `formatTokenCount()` (1.2M, 5.4k display) |
| `src/utils/time.ts` | `timeAgo()` (ISO string to relative "5m ago") |
| `src/utils/chartHelpers.ts` | `formatTime()`, `dedupeTickLabels()`, `anchorToNow()`, `getAreaColor()` |
| `src/utils/providers.ts` | `providerLabel()`, `normalizeProviderScope()`, `providerFilterLabel()`, `providerBadgeClass()` |
| `src/utils/retention.ts` | `retentionSpanFor()`, `markPrunedRange()`, `isPruned()`, `formatRetentionCutoff()`, `PRUNED_PLACEHOLDER` — see [[frontend#Frontend#Components#Retention Degradation]] |
