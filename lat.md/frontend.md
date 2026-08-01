# Frontend

The React 19 frontend is a multi-window Tauri application with custom hooks for IPC data fetching, an internal SVG kit for visualization, and pure CSS for styling.

## Entry Point

[[src/main.tsx]] routes to window-specific components based on the `?view=` URL parameter.

Each window gets its own Suspense boundary with a fallback. Per-window zoom persistence is stored in localStorage (`quill-zoom-{view}`) and supports Ctrl+/-, Ctrl+0 with a 0.5-2.0x range via Tauri's native webview zoom API, falling back to CSS `zoom` only outside Tauri. Ctrl+F is blocked to prevent the webview's native find-in-page (no search UI exists). A `ToastProvider` context wraps all views for notifications, [[src/hooks/useIntegrations.ts]] gates provider-dependent secondary windows when no provider is enabled, and [[src/windows/SessionsWindowView.tsx]] refreshes the session index on demand before loading search facets.

### Window Routes

Three Tauri windows are routed by the `?view=` URL parameter, each with its own Suspense boundary: the main widget, the consolidated Manage workspace, and the release-notes viewer.

| Route | Component | Purpose |
|-------|-----------|---------|
| `?view=main` (default) | [[src/App.tsx]] | The monitoring widget: titlebar, LIMITS band, switchable view region |
| `?view=manage` | [[src/windows/ManageWindowView.tsx]] | Rail-navigated Manage workspace; four tool UIs (Sessions, Learning, Instances, Settings) embedded as sections |
| `?view=release-notes` | `ReleaseNotesWindow` | Browse published GitHub release notes |

The former per-tool windows (`sessions`, `learning`, `restart`, `runs`, `settings`) were retired into Manage sections, and run history folded into the Learning section. All three remaining routes are reachable without an enabled provider — the Manage workspace gates each tool section inline (Settings always renders), so the former `BlockedWindow` per-window provider-blocking was removed.

Every route also mounts [[src/components/WindowResizeHandles.tsx]] as a sibling of its view, because all three windows are decorationless and none of them gets a resize border from the window manager. The main route takes the default `widget` geometry; `manage` and `release-notes` take the `roomy` variant.

## Manage Workspace

`?view=manage` ([[src/windows/ManageWindowView.tsx]]) is the management half of the monitor-vs-manage split: one rail-navigated window holding the former tool windows.

It is opened from the widget titlebar's un-gated settings key and by the app-scoped ⌘M / Ctrl+M accelerator.

The left rail has four flat sections — Sessions, Learning (Rules / Memory / Runs), Instances (instance restart), and Settings — with a signal-blue active indicator and a footer "Live" affordance back to the widget. The active section persists to `localStorage` (`quill-manage-section`) and accepts a `?section=` deep-link (the titlebar settings key opens `manage` at Settings). It still renders in the pre-widget roomier density on the Graphite Stack — the stated migration exception in DESIGN.md §6 — and converts to the flat plane in its own pass rather than piecemeal. Each section's content reuses the tool's existing window-view component, lazy-loaded and rendered with its own window chrome (titlebar/close) suppressed via `manage.css`; provider-dependent sections (Sessions, Learning, Instances) show an inline no-provider state while Settings stays reachable. The Learning section's Runs toggle opens run history as an inline right-docked panel (folded in from the former floating `runs` window). The standalone tool windows, their `?view=` routes, and capabilities entries have been retired. A rail Search affordance and `⌘K` / `Ctrl K` open the [[src/components/CommandPalette.tsx]] — a substring-filtered list of the four sections plus Back-to-Live and Close-Tools actions, navigated with arrow keys and Enter. The titlebar, its launcher button, and the palette's Close action display the label "Tools"; the window label, `?view=manage` route, `manage.css`, and component names are unchanged.

## Browser Mock Mode

In a plain browser during dev (no Tauri runtime), the app installs a mock IPC layer so it renders with fixture data instead of failing every `invoke()`. This is what lets `/impeccable live` drive the real app in a browser.

[[src/main.tsx]] checks `import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)` before any IPC runs and only then dynamically imports [[src/mocks/installBrowserMock.ts#installBrowserMock]]. The dynamic import plus the `DEV` guard keep the mock and its fixtures out of production builds entirely.

[[src/mocks/installBrowserMock.ts#installBrowserMock]] calls `mockWindows` and `mockIPC` from `@tauri-apps/api/mocks`, routing every `invoke()` to [[src/mocks/ipcFixtures.ts#handleInvoke]], and adds a fixed `MOCK DATA` badge. [[src/mocks/ipcFixtures.ts#handleInvoke]] returns typed sample data for the data commands (provider statuses with an enabled provider so the dashboard is not gated, usage buckets spanning the green/amber/red thresholds, token/code/breakdown/analytics datasets), benign defaults for Tauri core `plugin:*` commands so `listen()` resolves with events left inert, and `null` for anything unmapped.

Widget fixtures answer per range rather than serving one fixed window. `get_provider_token_series` returns aligned per-provider bucket values with per-provider totals whose sum equals the response total by construction, and `get_activity_series` returns aligned session and project counts; both honor the `range` (including the new `6h` step) and `buckets` arguments. `get_token_history` and `get_code_stats_history` now vary their point spacing for the hour-granular ranges instead of always returning the 24h shape; the 30-day token shape is daily and deliberately busier in the current week, so the Trends rows show real week-over-week movement in the browser. `get_llm_runtime_stats` scales with the requested window and its sparkline always sums to the total it reports, because consumers prorate that sparkline to recover a prior window's active seconds. The `retentionFixture=recent_watermark` control moves the mock watermark inside the last fortnight, which is how the Trends retention disclosure is reached in browser mode.

Model analytics mock handlers validate the same range, provider, and provider-qualified selection arguments as Tauri before applying dev-only failures. `modelFixture` selects lifecycle and exact empty-scope responses; retry keeps that scenario pending until another scenario or reload resets it. `modelFailure` rejects aggregate, history, session-page, session-detail, retry, or all commands through the shared structured envelope, and invalid nonempty controls warn and reject instead of silently falling back. Provider-qualified suppressed sources are removed before global or scoped facts. Opaque IDs remain dynamic evidence, not a support catalog.

Selected-model fixtures derive capped pages from observation data, including more than 20 matching sessions. Their opaque keyset cursor binds request identity to the final row's stable activity/provider/session tuple, rejects malformed, foreign, or stale anchors, and seeks past that tuple without offset drift. Lazy detail preserves parent/subagent boundaries, model gaps, repeated-model compression, Unicode-scalar primary ties, and one page-to-detail deletion returning bounded `not_found`.

A dev-only Vite plugin in [[vite.config.ts]] (`apply: "serve"`) relaxes the strict production CSP so the browser can load Vite HMR, React Fast Refresh, and the Impeccable live client at `http://localhost:8400`. Because it is serve-only, `vite build` never runs it and the shipped CSP is untouched.

## Main Window Layout

[[src/App.tsx]] is the widget shell: a freely resizable window holding [[src/components/widget/WidgetTitleBar.tsx]], a hairline, and one scrolling content column. The split-pane layout and its draggable divider were replaced by the widget redesign.

The shell owns only the app-lifecycle work that has no other home: usage polling every 3 minutes via `fetch_usage_data()` while a provider is enabled, the four-hour updater check, the right-click Refresh/Quit menu, and close-to-tray on both the titlebar control and the window manager's close request. Manual Refresh updates integration status; the polling effect owns the resulting usage fetch after that status settles, preventing duplicate requests. When no provider is enabled the column shows one shell-level empty state with a rescan action in place of every band.

Geometry is the user's, not the shell's. The window drags freely on both axes and `tauri.conf.json` declares floors (320x200) and no ceiling; the earlier content-derived height — a `ResizeObserver` that called `setSize` with the measured height and a fixed 360px width — was deleted rather than gated, because free resize and auto-height cannot both own the geometry and a dormant effect would reclaim the window on the first content change. `.wg-shell` fills the viewport at `height: 100vh`, and only the column below the titlebar scrolls, which is what keeps the widget reachable under webview zoom or when the user drags it shorter than its content.

Deleting the sizer left the config height as the only thing deciding how tall the widget opens, so that height is now measured against this shell rather than left at the sizer's old starting value: 43px of non-scrolling chrome plus the 745px the saturated Usage view renders at 360px wide, rounded to a 800px default and capped to the display on the launch that seeds it — see [[architecture#Multi-Window Design#Window Configuration#Seeded Height Clamp]]. The measurement is a one-off design input, not a runtime behaviour; nothing in the shell reads its own height any more.

The drag itself comes from [[src/components/WindowResizeHandles.tsx]], because a `decorations: false` window has no native border to grab — see [[architecture#Multi-Window Design#Window Configuration]]. [[src/main.tsx]] mounts it beside `App` in its default `widget` geometry; the other two routes mount the same component in the `roomy` variant.

### Component Tree

The widget nests [[src/components/widget/WidgetTitleBar.tsx#WidgetTitleBar]] above a scrolling content column holding [[src/components/widget/LimitsSection.tsx]] and, below it, the switchable view region ([[lat.md/frontend#Frontend#Components#Widget View Region]]).

`WidgetTitleBar` carries the widget's whole chrome contract in three grid tracks: the brand glyph and `QUILL` wordmark on the left, the update button centred on the window (rendered only once the updater check has found a release, wired to `install_app_update`), and a right cluster of the sync freshness pill, the always-on-top toggle, the settings key, and the close key. The bar and its non-interactive children carry `data-tauri-drag-region`, so a decorationless window stays draggable. The sync pill is a `role="status"` `aria-live="polite"` region showing real elapsed time since the last successful read; it absorbs the old usage pill vocabulary as slate variants (`offline` beats `cached` beats `paused`, mirroring the precedence those pills used) and never turns red. The always-on-top toggle reads and writes the persisted `always_on_top` setting through [[src/hooks/useRuntimeSettings.ts#useRuntimeSettings]], so the tray checkitem, the Settings toggle, and the titlebar are one state; a failed write surfaces as a toast rather than a button that lies. Both the settings key and the ⌘M/Ctrl+M accelerator registered in [[src/main.tsx]] go through [[src/lib/manageWindow.ts#openManageWindow]], which focuses an existing Manage window instead of stacking a second one.

## Components

Components are organized by feature domain under `src/components/`.

### Core Components

Shared chrome, dialogs, and disclosures that are not owned by a single window.

The widget's own chrome lives under `src/components/widget/` and is described in [[lat.md/frontend#Frontend#Main Window Layout#Component Tree]]; everything below is used by two or more surfaces.

- **ReleaseNotesWindow** (`src/windows/ReleaseNotesWindow.tsx`) — Standalone window that fetches published GitHub releases through the [[src-tauri/src/lib.rs#get_release_notes]] command, shows the latest first, and places Previous/Next navigation plus the selectable release URL in a top toolbar below the titlebar. Centers the release tag between the release counter and publish date, renders release bodies as sanitized GitHub-flavored Markdown that fills the scroll area, surfaces loading, empty, and error states with a Retry control, and supports Escape plus Left/Right arrow keyboard navigation. Its only entry point is the About row in the General settings tab — the widget titlebar carries no version affordance.
- **ConfirmDialog** ([[src/components/ConfirmDialog.tsx]]) — Shared confirmation modal used for destructive provider cleanup and provider installation confirmation, driven from the Integrations settings tab.
- **CommandPalette** ([[src/components/CommandPalette.tsx]]) — The Manage workspace's `⌘K` / `Ctrl K` substring-filtered navigator over its four sections plus Back-to-Live and Close-Tools actions.
- **WindowResizeHandles** ([[src/components/WindowResizeHandles.tsx]]) — The resize border shared by all three decorationless windows: eight absolutely-positioned zones (four edges, four corners, the corners painted last so they win the diagonal cursor) inside a click-through fixed overlay, each handing its gesture to the compositor through `startResizeDragging` so the drag stays native and never round-trips through React. They are `aria-hidden`, pointer-only, hold no focusable content, and act on the left button alone so the widget's right-click Refresh/Quit menu still opens. The `variant` prop picks the geometry, which is expressed as the `--wrh-edge` and `--wrh-corner` custom properties on the overlay: `widget` (default) is 5px edges and 12px corners, `roomy` keeps the 5px edges and drops to 8px corners for Manage and release-notes. Both mounts are in [[src/main.tsx]] — see [[architecture#Multi-Window Design#Window Configuration#Resize Border Geometry]] for why the two windows cannot share one number.
- **RetentionBanner** ([[src/components/RetentionBanner.tsx#RetentionBanner]]) — The multi-line retention disclosure, described in [[lat.md/frontend#Frontend#Components#Retention Degradation]]. The widget states the same fact as a condensed one-line variant inside the affected view rather than mounting this banner.

The pre-widget main-window chrome — `TitleBar`, `UsageDisplay`, `UsageRow`, the `live/` modules, and the `ProviderMenu` popover with its legacy `IntegrationsWindow` host — was deleted with the widget redesign. Their responsibilities moved to [[src/components/widget/WidgetTitleBar.tsx#WidgetTitleBar]], [[src/components/widget/LimitsSection.tsx#LimitsSection]], and the Settings surfaces respectively.

### Widget Viz Kit

Three SVG primitives under `src/components/widget/viz/` draw every widget chart with no charting dependency, so the Flat Polish treatments (surface-faded overlay, hover-only legend, endpoint markers) stay under direct control.

Recharts is hostile to those treatments and costs 3.7 MB for shapes the widget can emit in ~300 lines, so the kit owns all widget visualization. The primitives take plain value arrays and a colour, never a data-frame; scaling and curve construction live in one shared module.

- **Sparkline** — [[src/components/widget/viz/Sparkline.tsx#Sparkline]] renders the 13px trend line under each readout cell: metric-hue stroke at 60% opacity plus a solid endpoint dot, no axes or ticks. Without a `label` prop it is `aria-hidden`, because the adjacent readout already states the number.
- **AreaChart** — [[src/components/widget/viz/AreaChart.tsx#AreaChart]] stacks one smoothed area per provider on a shared scale, keeps the series inside the lower ~62% so the overlaid headline never collides, and exposes an `overlay` slot plus a legend chip that is hidden at rest and revealed on hover or focus. Gradient ids are derived from `useId()` with punctuation stripped so `url(#…)` stays valid. A range with no values renders the shared empty state at the chart's own height, so nothing below shifts.
- **Bars** — [[src/components/widget/viz/Bars.tsx#Bars]] renders horizontal magnitude rows on a shared scale; each track is a real `role="progressbar"` with `aria-valuenow`/`min`/`max` and a formatted `aria-valuetext`, so the value is announced rather than inferred from pixel width.
- **geometry** — [[src/components/widget/viz/geometry.ts#scalePoints]] maps values into viewBox coordinates and [[src/components/widget/viz/geometry.ts#smoothPath]] builds the catmull-rom-to-bezier curve used by `specs/018-widget-ui-redesign/mockup.tpl.html`, so shipped charts trace the mockup's silhouette. Every helper returns new arrays or strings and never mutates its input.

### Widget Limits Band

[[src/components/widget/LimitsSection.tsx#LimitsSection]] is the widget's whole subscription readout: one row per enabled provider, and no section at all when none is enabled.

Each row is an identity swatch (the fixed provider hue), the provider name, one fixed-width cell per rate-limit window, and a right-aligned countdown to that row's nearest upcoming reset. The cells hold their column width so rows scan as a table at 360px whatever their bucket count, shrinking only when a provider reports more windows than the row can seat. A cell states the rounded percent, a window label compressed from the bucket label (the untouched label stays in the cell title and the bar's accessible name), and a 4px `role="progressbar"` bar carrying `aria-valuenow`/`min`/`max`.

Severity is carried on `[data-severity]` and follows the same 50/80 thresholds as [[lat.md/features#Features#Live Usage View]]: amber from 50%, red from 80%. A bucket whose `resets_at` has already elapsed is marked stale instead, which matches no severity rule and therefore renders neutral — a utilization measured against a bygone window must never read as a live severity. For the same reason an elapsed window is not a candidate for the row's nearest-reset countdown; the countdown falls back to "now" only when every dated window in the row has rolled over.

A provider with no live buckets still gets a row, stating why in the app's existing pill wording but in the widget's flat dress (a lamp and a word, no box): `SETUP` in amber when the failure is actionable — a `config`/`auth` provider error, or an unfinished install — and `UNAVAILABLE` in slate otherwise, since a degraded read is never an alarm. Before the first usage poll lands the row shows skeleton cells of the same geometry, so the real numbers do not move the rows beneath them. MiniMax rows keep the plan-level bucket filter (M\*, coding-plan-search, coding-plan-vlm) that the deleted `ProviderUsageModule` applied, because the per-model long tail does not fit a 360px row.

### Widget View Region

[[src/components/widget/ViewRegion.tsx#ViewRegion]] owns everything below LIMITS: one band header carrying the view name and the shared range strip, then whichever view that name selects.

View and range both live in the region rather than inside a view, so switching views keeps the operator's range and the mockup's single control strip stays single. A compact view is registered by adding one entry to the region's `VIEWS` list; only registered views reach the dropdown, so the list can never offer a view that would render nothing. The range vocabulary is 1H/6H/24H/7D — `30d` is deliberately absent, because a month is not a widget scope — and defaults to 6H, the shortest window that still spans a working session.

A registered view may declare itself unranged. The range strip is then hidden for as long as that view is showing, rather than left standing as a control that would change nothing: Trends fixes its own windows, and an inert toggle strip would be a lie about what the instrument responds to.

[[src/components/widget/ViewSwitcher.tsx#ViewSwitcher]] is a listbox rather than a menu, because the control has a value: the trigger is `aria-haspopup="listbox"` with `aria-expanded`, the popup carries exactly one `aria-selected` option, and keyboard movement runs through `aria-activedescendant` so focus never leaves the list. Escape, Tab and an outside click all close it, and Escape returns focus to the trigger.

#### Usage View

[[src/components/widget/views/UsageView.tsx#UsageView]] is the widget's default view and the product's core surface: hero chart, insight line, a 3×2 readout grid, the switchable breakdown, and the totals footer.

Every band reads the region's selected range, so chart, delta, insight, all six readouts, all six sparklines and the footer always describe the same window — a band quietly using a different window would be a lie about the instrument. The headline overlaid on the chart is `total_tokens` from [[src/hooks/useWidgetSeries.ts#useProviderTokenSeries]], which is the same figure the plotted areas sum to by construction. Its delta is momentum *inside* the range (the back half of the buckets against the front half) rather than a comparison with the previous window: a headline delta whose evidence the chart does not draw is not evidence.

Colour carries meaning and nothing else. Each readout's fixed metric hue appears only on its label swatch, its sparkline stroke and that stroke's endpoint; values stay `--text-hi`. Green and red on a delta are assigned by *meaning*, never by arrow direction — `InsightTrend.upIsGood` decides, so a falling tokens-per-LOC reads as the improvement it is, and a trend whose goodness is unknown stays neutral.

Per-metric sources: runtime and its sparkline from [[src/hooks/useLlmRuntimeStats.ts#useLlmRuntimeStats]]; tokens-per-LOC and LOC-per-hour with their trends from [[src/hooks/useCodeInsights.ts#useCodeInsights]]; sessions and projects sparklines from [[src/hooks/useWidgetSeries.ts#useActivitySeries]]; net lines from [[src/hooks/useCodeStats.ts#useCodeStats]], bucketed as `lines_added − lines_removed`. The footer's In/Out/Cache totals come from a range-scoped `get_token_stats` read rather than the point-history-plus-hostnames hook the deleted analytics pane used, because a background instrument should not pay for the point history and hostname list it never draws.

The insight line carries one computed insight per window, chosen by the rotation rule in [[src/components/widget/views/insightLine.ts#selectInsightLine]] rather than pinned to a single source. Every candidate restates figures the view already read for that same window: context savings from [[src/hooks/useContextSavingsStats.ts#useContextSavingsStats]], the cached-token volume behind the footer's `Cache` percentage, and the per-provider split behind the hero chart. A candidate speaks only when its figure exists and is non-zero, so nothing is ever zeroed or padded to keep the line occupied; with no eligible candidate the line is simply not drawn.

Priority is fixed and ordered by how much of the story the rest of the widget does not already tell — savings appears nowhere else, the cached-token volume is only implied by the footer, and the provider split is already drawn directly above, so it speaks last. The first eligible candidate wins, and a higher-priority candidate whose source has not answered yet holds the line empty rather than letting a lower one appear and be swapped out a moment later; a failed read counts as answered-with-nothing, so one broken source cannot mute the line. Selection is a pure function of the window and its resolved data — no clock and no counter rotates it under the reader, so the same window with the same data always states the same thing.

The breakdown switches five modes over one row grammar — status dot, name, identity chip, dim secondary count, primary value, recency — filled per mode: Sessions (provider chip, tokens, live count in the header), Projects (session count, tokens), Hosts (turns, tokens), Skills (uses, last used), Hooks (QUILL chip where Quill-deployed, fires, last fired). Honesty disclosures keep the home that matches their data: the Hooks header carries the Claude/Codex tracking-asymmetry help, and the condensed retention line sits in Sessions, the only mode whose source [[lat.md/frontend#Frontend#Components#Retention Degradation]] actually prunes — skill and hook counts are never pruned, so claiming loss there would be its own lie.

#### Trends View

[[src/components/widget/views/TrendsView.tsx#TrendsView]] is the widget's slow instrument: three week-over-week rows — tokens, velocity, cache efficiency — each carrying this week's value, the delta against last week, and the paired mini-bars behind it.

The windows are the last seven days against the seven before them and are not re-scopable, which is why the view registers itself unranged. Every figure comes from [[src/hooks/useWeeklyTrends.ts#useWeeklyTrends]], which reads the 30-day `get_token_history`, `get_code_stats_history` and `get_llm_runtime_stats` the app already ships and splits them at the week boundary, so the numbers agree with the surfaces that show them elsewhere. Velocity divides by [[src/hooks/useCodeInsights.ts#computeVelocity]]'s active-runtime denominator — the same definition the Usage readout prints — and recovers the prior week's active seconds by prorating the 30-day runtime sparkline, because `get_llm_runtime_stats` accepts only fixed ranges and cannot query the week before last directly.

Both sides or neither: a delta renders only when both weeks have a figure, since a percentage against an absent week is invented movement. Colour stays on the delta and is assigned by meaning — rising velocity and rising cache efficiency read green, while token volume, being neither good nor bad, stays neutral. The bars carry no hue at all; this week is bright and last week is dim, on one shared scale.

Only velocity degrades under retention: it reads `tool_actions` and `session_events`, both of which [[lat.md/frontend#Frontend#Components#Retention Degradation]] prunes, while the token figures come from snapshots retention never touches. A compared week that falls entirely below the watermark renders as an em dash with no delta rather than as a collapse in output, and a condensed line beneath the rows states the cutoff whenever either week is affected.

#### Charts View

[[src/components/widget/views/ChartsView.tsx#ChartsView]] stacks tokens, code changes and cache efficiency on one time axis under one crosshair, so the operator can read the three against each other rather than one at a time.

Correlation is the view's only reason to exist, so every panel is bucketed onto the timestamps `get_provider_token_series` returns for the region's range. The code and cache sources arrive on their own server-side granularity and are re-gridded by [[src/components/widget/views/ChartsView.tsx#bucketAssigner]], which reads the bucket width from the grid itself instead of recomputing it — a panel drawn on a different grid would make the shared crosshair assert an alignment that does not exist. Cache rates come from summed numerators and denominators per bucket rather than averaged point rates, so a busy minute is not outvoted by an idle one and the range figure is the same weighted rate the Usage footer reports.

There is no floating tooltip. Scrubbing — pointer, or arrow keys once the stack has focus — swaps each panel's head value in place and brightens the matching axis tick, which makes the axis itself the time readout and costs no layout shift. The stack is a labelled `role="group"` with a screen-reader instruction and a polite live region announcing the bucket's three values together. Escape clears the reading, a range change drops it because the grid is rebuilt, and every head reads `—` until its own source lands so no panel states a zero it has not measured.

Because that brightened tick *is* the readout, no two ticks may read alike, and [[src/components/widget/views/ChartsView.tsx#axisLabels]] derives the caption format from the returned grid rather than from the range name. A window inside a day is unambiguous as `HH:MM`; a window spanning days takes the weekday, and a grid whose buckets are narrower than a day takes the hour on top of it. 7D is exactly that case — eight buckets across seven days sit 21h apart, so bare weekdays repeat and the axis would assert one bucket while the crosshair read another. Ticks never wrap, so a longer caption cannot change the row's height.

Gaps stay gaps: a bucket with no cacheable input has no hit rate, so the cache line breaks over it instead of drawing 0%, while tokens and code are genuinely zero when nothing happened and are drawn as zero. Failure is region-local — a failed code or cache read leaves its own panel stating so while the others keep charting — except for the token series itself, which is the shared grid, so losing it collapses the band to one line rather than showing three panels that cannot be compared.

Colour follows [[lat.md/frontend#Frontend#Styling#Color System]]: tokens carry provider identity, and because added-versus-removed lines are a category rather than a threshold state, the diff pair is drawn from the metric ramp (`--metric-net-lines` up, `--metric-loc-per-hr` down) instead of the conventional green/red the severity rule reserves. Cache takes the widget's throughput cyan. The condensed retention line appears under the panels only when the watermark actually falls inside the drawn window, because `get_code_stats_history` reads the `tool_actions` rows [[lat.md/frontend#Frontend#Components#Retention Degradation]] prunes.

#### Models View

[[src/components/widget/views/ModelsView.tsx#ModelsView]] answers "what am I running, and what did the work" in two bands: a running-now strip, then the session-ranked model list.

Both bands read the region's range through [[src/hooks/useModelAnalytics.ts#useModelAnalytics]], so one usage-overview snapshot serves the whole view and the hook's own coalescing — a one-second window after a committed model event, a 60-second fallback poll — keeps a background instrument off the backend's back. There is no inspect panel: session paging and chain history stay with the full page, so a widget row is deliberately inert.

Identity obeys DESIGN.md's Model-Shade Rule exactly as the full page does. Each model renders as a rank-assigned shade of its provider's family ramp (Claude orange, Codex blue, every other provider violet, rank seven and beyond neutral), assigned once per response from the delivered session-ranked order so both bands agree on a model's shade. A swatch never stands alone — it rides beside the raw id, qualified by a provider chip, and an unrecognized provider keeps a neutral chip rather than borrowing another family's hue. Ids are mono, rendered exactly as observed and ellipsized when they outgrow the column with the full string in `title`; no catalog, alias, or friendly name participates. The ranked list shows the top five models on one shared session scale, each track a real `role="progressbar"`, with attributed tokens beside the id.

A ranked row only exists because its model ran sessions, and a session that ran necessarily burned tokens — so a zero attributed-token figure is the absence of a measurement rather than a measurement of nothing, and it is what a provider whose observations carry no token columns looks like. [[src/components/widget/views/ModelsView.tsx#tokenReading]] therefore prints an em dash with the reason on hover instead of `0`, the same way every panel head states a figure it does not have. Times in both bands are 24-hour, matching the Usage readouts and the Charts axis; a 12-hour caption would print `05:39 PM` for the instant the axis calls `17:40`, and its meridiem suffix is not a tabular figure.

Two disclosures keep a compact home here. Coverage states the share of token activity that carries model evidence whenever it is short of 100%, because activity recorded before a chain's first observation stays unattributed instead of being assigned a model. A retained-history line appears only while the backfill needs attention, carrying its state, its processed-source count, and a Retry while the run is retryable; a refused retry states its reason rather than vanishing. Emptiness is a claim the view has to earn: it names the specific negative — no retained sessions, no sessions in range, or sessions carrying no model identifier — only when the backend calls the scope final and the history inventory complete, and otherwise says the evidence is still being processed.

#### Context View

[[src/components/widget/views/ContextView.tsx#ContextView]] states what the working-context store did with the selected range: preserved and retrieved token headlines, the shared savings line, and what routing cost.

The view is deliberately chartless. Its `summary` totals and the per-bucket `timeSeries` are computed from different token columns — the category-scoped totals the savings taxonomy introduced versus the legacy per-bucket estimates that also counted telemetry — so plotting the series beneath these headlines would put two disagreeing numbers in one band, and a headline the graphic contradicts is worse than no graphic. The single visualization is a split bar assembled from the exact three figures printed around it: how the range's accounted context tokens divide between preservation, retrieval and routing.

Only category-scoped totals are read, never the legacy `tokens*Est` columns, because those counted telemetry as savings and quoting them here would re-inflate the very headline the taxonomy exists to correct. A backend that does not categorize therefore reads as zero and the view says which nothing it is looking at — "context events recorded, none carrying token categories" is a different fact from "no context events in this range". The guidance-event count follows the same rule: [[src/hooks/useContextSavingsStats.ts#useContextSavingsStats]] normalizes an absent category count to zero, so a zero beside non-zero routing tokens can only mean unreported and falls back to the event-type-scoped `routerEventCount`; with neither count present the clause is dropped rather than printed as a zero that contradicts the value next to it.

The savings sentence comes from [[src/components/widget/views/insightLine.ts#contextSavingsInsight]] — the same builder behind the Usage view's first insight candidate — so the two surfaces cannot word or round one claim differently. Here it is unconditional rather than one candidate among several: on this view the context store is the subject, not one story competing with two others.

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
`sessions`) and `get_batch_session_code_stats` (surface `session-search`).
Session search still mounts the full banner from
[[src/windows/SessionsWindowView.tsx]]; the widget has no room for a
multi-line disclosure, so its Sessions breakdown, Trends rows, and Charts
panels state the same cutoff as a condensed one-line variant placed in the
band whose own source is pruned. Styling is chrome-grey by design:
DESIGN.md reserves green/amber/red for the severity meter, and a boundary the
user opted into is a fact about the instrument, not an alarm.

[[src/utils/retention.ts]] holds the pure helpers.
[[src/utils/retention.ts#retentionSpanFor]] classifies a `[first_seen,
last_active]` span as `retained` / `straddles` / `pruned`;
[[src/utils/retention.ts#isPruned]] is the single-instant form;
[[src/utils/retention.ts#formatRetentionCutoff]] renders the watermark date; and
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

Four hooks back the [[features#Settings Window]]: each owns one slice of state, calls Tauri IPC for mutations, and subscribes to the matching push event so the Settings surface and the widget titlebar stay in sync.

| Hook | File | Source of truth | Listens for |
|------|------|-----------------|-------------|
| `useIntegrationFeatures` | [[src/hooks/useIntegrationFeatures.ts]] | `IntegrationFeatures` global flags (context preservation, activity tracking, context telemetry) | `integration-features-updated` |
| `useRuntimeSettings` | [[src/hooks/useRuntimeSettings.ts]] | `RuntimeSettings` background-task tunings (live-usage interval, rule watcher, always-on-top) | `runtime-settings-updated` |
| `useLearningSettings` | [[src/hooks/useLearningSettings.ts]] | `LearningSettings` (trigger mode, periodic interval, thresholds) | None — read on mount and after save |
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
[[frontend#Frontend#Components#Retention Degradation]].

The fifth hook, `useUiPrefs`, is gone. It carried the split-pane layout mode, the usage-row time mode, and Live/Analytics panel visibility — every one of which the widget redesign deleted — so the hook, the `UiPrefs` type, its Settings controls, and the frontend-emitted `ui-prefs-updated` event were removed together rather than left as settings that configure nothing.

### Data Fetching Hooks

Hooks that invoke Tauri commands and return async state (data, loading, error).

| Hook | Returns | Tauri Commands |
|------|---------|----------------|
| `useProviderTokenSeries` | Aligned per-provider token series plus per-provider totals for the widget hero chart, on the shared 8-bucket grid | `get_provider_token_series` |
| `useActivitySeries` | Per-bucket distinct session and project counts for the widget sparklines, on the same grid | `get_activity_series` |
| `useWeeklyTrends` | Week-over-week tokens, velocity and cache efficiency for the widget Trends view, with each week classified against the retention watermark | `get_token_history`, `get_code_stats_history`, `get_llm_runtime_stats` |
| `useCodeStats` | Lines added/removed by language, plus the bucketed history the net-lines sparkline reads | `get_code_stats`, `get_code_stats_history` |
| `useCodeInsights` | Tokens-per-LOC and LOC-per-active-hour with their trends, over one shared comparison-range fetch | `get_code_stats_history`, `get_llm_runtime_stats`, `get_token_history` |
| `useLlmRuntimeStats` | Cumulative runtime, session count, turn count, avg per turn, sparkline | `get_llm_runtime_stats` |
| `useBreakdownData` | Session/project/host/skill/hook breakdown rows for the widget's five breakdown modes | `get_session_breakdown`, `get_project_breakdown`, `get_host_breakdown`, `get_skill_breakdown`, `get_hook_breakdown` |
| `useModelAnalytics` | One model usage-overview snapshot per scope plus backfill state — see [[frontend#Frontend#Custom Hooks#Model Analytics Hook]] | `get_model_usage_overview`, `retry_model_history_backfill` |
| `useContextSavingsStats` | Context savings summary, category-scoped time series, breakdowns, and recent events; subscribes to `context-savings-updated`. Powers the [[features#Widget Views#Context View]] and the Usage view's first insight candidate. | `get_context_savings_analytics` |
| `useRetentionCutoff` | Read-only retention watermark + window for the degradation treatment; re-reads on `retention-maintenance-finished` | `get_retention_policy` |
| `useSessionCodeStats` | Batch LOC stats per session (ref-cached) for session search | `get_batch_session_code_stats` |
| `useLearningData` | Rules, runs, settings, observations, logs | Multiple learning commands + events |
| `useMemoryData` | Memory files, suggestions, projects | Multiple memory optimizer commands |

The hooks the deleted analytics pane owned — `useAnalyticsData`, `useTokenData`, `useLiveSummaryData`, `useSessionHealth`, `useActivityPattern`, `useEfficiencyStats`, `useVelocityStats`, `useCacheEfficiency`, `useSessionSubagents`, `useSkillProjects`, `useModelSessions`, `useSessionModelHistory` — were removed with it. The widget reads `get_token_stats` directly for its footer totals rather than reviving a history hook for three numbers.

[[src/hooks/useCachedInvoke.ts#useCachedInvoke]] is the shared cache primitive
for `useModelAnalytics`, `useWidgetSeries`, `useWeeklyTrends`, `useCodeStats`,
`useCodeInsights`, `useLlmRuntimeStats`, `useContextSavingsStats`, and
`useBreakdownData`. Each hook keeps identity-scoped accepted data, starts its
first request immediately, and debounces later refreshes by 200 ms. A
generation guard discards stale responses, a same-identity in-flight request
coalesces refreshes, and equal JSON results retain their prior object identity.
This gives every hook stale-while-revalidate rendering without duplicating
request-lifecycle code — which is what lets a view switch back to a range it
already visited without a skeleton.

Data hooks subscribe to backend push events instead of relying only on the 60-second polling fallback. `useLlmRuntimeStats` and `useBreakdownData` refresh on `sessions-index-updated`; `useCodeStats` and `useCodeInsights` also subscribe to `transcript-analytics-updated`, because after migration 30 `tool_actions` is written exclusively by source-owned reconciliation and `sessions-index-updated` no longer covers it. `useCodeInsights` additionally listens to `tokens-updated` because it combines code and token history. The widget series hooks in [[src/hooks/useWidgetSeries.ts]] share one refresh path — both read `token_snapshots`, so both debounce on `tokens-updated` and poll on the same 60-second interval, and neither can end up a refresh behind the other.

`useMemoryData` tracks concurrent optimization runs by run id and uses background refreshes for event-driven updates so `Optimize All` does not drop out of the running state or flash the all-projects view on every completion event. The hook initializes the Memories tab to the aggregate `__all__` selection on first load, then reuses the project-scoped delete IPC command to support current-view bulk deletion in both single-project and all-projects modes.


### State Pattern

Hooks follow a consistent async state pattern: `useState` for data/loading/error, `useRef` for initial load tracking, `useEffect` for fetching, periodic interval refresh, and Tauri event listener cleanup.

### Model Analytics Hook

`useModelAnalytics` keeps usage-overview and backfill retry state independent so a failed refresh cannot replace the last successfully loaded same-scope overview.

[[src/hooks/useModelAnalytics.ts#useModelAnalytics]] takes `(range, provider, active)`, fetches the single `get_model_usage_overview` snapshot per scope, and exposes separate initial-loading, refresh-loading, structured-error, and Retry state. [[src/hooks/useCachedInvoke.ts#useCachedInvoke]] owns its identity-keyed cache, first-call-immediate/later-call-200ms debounce, stale-response generation guard, same-identity deferred refresh dedupe, and byte-identical reference reuse. Revisiting a range/provider scope therefore renders cached data while it revalidates without a skeleton. Backfill status persists across scope changes from accepted snapshots and the structured retry response; generation, lifecycle, inventory, and monotonic progress outrank wall-clock timestamps so clock rollback cannot hide completion. Its own guarded Retry never clears recovered overview data. External refreshes are gated: the `model-analytics-updated` listener ignores events whose payload reports `dataChanged === false`, and event-driven refresh and the 60-second poll pause while the panel is inactive or the document is hidden, replaying a single missed signal once on re-activation. One Strict Mode-safe listener still starts a fixed one-second deadline at the first observable event and reconciles once when its asynchronous registration becomes active to close the initial fetch/subscription gap unless a captured event already owns that refresh; a disposed registration only unsubscribes. Model selection is client-side and never refetches the overview; the widget's [[lat.md/frontend#Frontend#Components#Widget View Region#Models View]] has no inspect panel, so one overview snapshot serves the whole view and there are no detail hooks to fan a refresh generation out to.

### Context

React Context providers used across the frontend for shared state.

- **ToastProvider** (`src/hooks/useToast.tsx`) — Notification system via React Context. Provides `toast(level, message)` to any component.

There is no crosshair context any more: the widget's Charts view keeps its three panels on one grid and one local scrub index inside [[src/components/widget/views/ChartsView.tsx#ChartsView]], so cross-chart synchronization needs no shared provider.

## Type Definitions

[[src/types.ts]] contains shared TypeScript types mirroring the Rust models in [[src-tauri/src/models.rs]].

Key type categories: usage/token tracking (`UsageBucket`, `TokenDataPoint`, `TokenStats`, `ProviderCredits`), context savings (`ContextSavingsAnalytics`, `ContextSavingsEvent`), indicator state (`IndicatorPrimaryProvider`, `IndicatorMetric`, `StatusIndicatorState`), analytics (`BucketStats`, `SessionHealthStats`, `ResponseTimeStats`), learning (`LearnedRule`, `LearningRun`, `LearningSettings`), session search (`SearchHit`, `SearchResults`, `SessionContext`), restart (`ClaudeInstance`, `RestartStatus`), memory (`MemoryFile`, `OptimizationSuggestion`).

Display enums: `RangeType` (now carrying the widget's `6h` step), `TrendType`, `BreakdownMode`, `SortMode`. `TimeMode` and `AnalyticsTab` went with the usage-row time modes and the analytics tab bar. `RangeType` carries the all-range retention invariant in its doc comment, and `SessionBreakdown.subagent_count` / `SessionCodeStats` carry their retention degradation notes — both described in [[frontend#Frontend#Components#Retention Degradation]].

Retention types (`RetentionPolicy`, `RetentionPreview`, `RetentionAuditRecord`, `RetentionMaintenanceProgress`, `RetentionMaintenanceResult`) mirror [[src-tauri/src/retention.rs]] and keep snake_case because they arrive straight off `invoke()` with no mapping layer.

## Styling

Pure CSS with no framework, organized around a `:root` design-token layer in `src/styles/index.css` per DESIGN.md.

The widget paints the Flat Polish surface `--surface` (`#14181f`) with the `--text` (`#c9d1d9`) ladder at a 10px base; the windows DESIGN.md §6 defers still paint the `--console-black` (`#121216`) canvas at 11px.

### Typography

Body/UI text is **Geist** and monospace contexts (ids, code, paths) are **Geist Mono** — both self-hosted variable fonts (weights 100–900) with system stacks as fallback.

Both are vendored from the `geist` npm package into `src/assets/fonts/` (`Geist-Variable.woff2`, `GeistMono-Variable.woff2`) and declared via `@font-face` in `index.css` with `font-display: swap`. Every window stylesheet's mono stack leads with `"Geist Mono"`.

### Design Tokens

The canonical palette lives as `:root` CSS custom properties in `src/styles/index.css`, following DESIGN.md. Because [[src/main.tsx]] loads `index.css` for every window, these tokens are global to all stylesheets.

Tokens cover backgrounds (`--console-black`, `--panel-deep`, `--panel-raised`, `--card-graphite`, `--slate-input`, `--graphite-line`), the Graphite Stack line ladder (`--hairline`, `--hairline-strong`) and its interaction fills (`--fill-ghost`, `--fill-hover`), text (`--readout`, `--readout-bright`, `--label`, `--label-faint`), the status meter (`--meter-green` / `--meter-amber` / `--meter-red`), accents (`--signal-blue` / `--signal-cyan` / `--signal-violet` / `--signal-orchid`), provider identity (`--provider-claude` / `--provider-codex` / `--provider-minimax` / `--provider-agent`), and `--radius-*` / `--space-*` scales. Every window stylesheet reads its palette from these vars. The former Tokyo-night palette and divergent green and lifecycle colors, plus the GitHub-dark insight-card/tooltip sub-palette and assorted near-whites (`index.css`, `settings.css`) have all been unified onto the canonical tokens. The only remaining color literals are neutral white/black alpha — the dimming ladder — and one intentional lighter-green toggle-hover tint.

`--hairline` and `--hairline-strong` are the line ladder for the windows DESIGN.md §6 defers — Manage, the learning section, and release-notes. `--hairline` is an alias of `--graphite-line` (`#21262d`) rather than the Flat Polish `--line`, because §6 forbids back-porting Flat Polish tokens piecemeal into a legacy window: a window converts in one pass or not at all. `--hairline-strong` (`rgba(255, 255, 255, 0.18)`) is the hover step under it. Both were referenced by `manage.css` and `learning.css` long before they were defined, and an undefined `var()` makes the whole declaration compute to its initial value — so the Manage outer border, its titlebar rule, and the rail divider were silently dropped at every size until the tokens landed.

`--fill-ghost` (`rgba(255, 255, 255, 0.04)`) and `--fill-hover` (`rgba(255, 255, 255, 0.08)`) are the interaction fills under that same line ladder, and share its reasoning: declared as independent white-alpha steps rather than aliased to the Flat Polish `--hover`, because §6 forbids piecemeal back-porting. Despite the names, `--fill-ghost` is the lighter *hover* step and `--fill-hover` the heavier *selected/active* step — 4% is the DESIGN.md §2 hover fill and 8% its "selected is an 8% white fill" rule, giving a 4 → 8 → 18 alpha ladder with `--hairline-strong`. Both stay grayscale, so a selected row never competes with the severity meter or the signal-blue active indicator. They too were referenced before they were defined — and unlike the hairlines, every reference was bare with no `var()` fallback, so the Manage rail hover and active item, the ⌘K palette's active row, the Manage and learning close buttons, and the rail back button painted no fill at all: selection was carried only by the text-color shift and the blue left-edge indicator.

A second token block in the same `:root` carries the Flat Polish system that DESIGN.md now describes as the whole app's target: the flat surface pair (`--surface` `#14181f`, `--inset`), hairline and hover alphas (`--line`, `--line-soft`, `--hover`), a brightness-only text ladder (`--text-hi`, `--text`, `--faint`), six metric hues (`--metric-runtime`, `--metric-tok-per-loc`, `--metric-loc-per-hr`, `--metric-sessions`, `--metric-projects`, `--metric-net-lines`), and three context-category hues (`--context-preserved`, `--context-retrieved`, `--context-routing`). The metric and context hues are permitted on sparkline strokes, endpoints, label swatches, and split-bar segments only — values stay `--text-hi` and severity stays with the meter, so identity never reads as state. Provider hues are reused unchanged from the block above. The Graphite Stack block survives only for the windows DESIGN.md §6 defers; new surfaces are built on `--surface`.

Alongside them, `index.css` defines the widget's shared primitives: `.wg-key` keycaps, `.wg-toggles`/`.wg-toggle` strips, the `.wg-pill` sync/status pill, `.wg-rule` hairline, `.wg-state` per-band empty/loading/error boxes, `.wg-skeleton` shapes, the `.wg-bar` utilization bar, and the `viz-*` classes the kit renders into. Accessibility is enforced through the selectors rather than left to callers: a toggle only renders as selected under `[aria-pressed="true"]`, the pill's degraded variants key off `data-state`, and the pulse and skeleton animations are wrapped in `prefers-reduced-motion: no-preference`.

### Stylesheets

Per-window CSS files under `src/styles/`, each scoped to a specific feature domain.

| File | Lines | Scope |
|------|-------|-------|
| `src/styles/index.css` | 2,697 | Design tokens, shared chrome, and every widget section |
| `src/styles/learning.css` | 1,120 | Learning section and components |
| `src/styles/settings.css` | 628 | Settings tabs |
| `src/styles/sessions.css` | 511 | Session search section |
| `src/styles/manage.css` | 469 | Manage workspace rail and embedded-section chrome |
| `src/styles/restart.css` | 356 | Instances (restart) section |

Widget styles live as sections inside `index.css` rather than a separate `widget.css`: one global stylesheet is a constitution constraint, and the widget's rules need the same `:root` tokens every other window reads.

### Color System

Semantic palette, drawn from the `:root` tokens. Status color is reserved; identity color is fixed per provider.

- **Status meter** (`--meter-green` `#34d399` < 50%, `--meter-amber` `#fbbf24` 50-80%, `--meter-red` `#f87171` >= 80%): utilization, trends, success/warning/error. Reserved for threshold state only.
- **Signal blue** (`--signal-blue` `#60a5fa`): accents, selection, focus rings, primary actions. The sessions search/filter focus and active-sort toggle use this — previously green, which collided with the meter.
- **Provider identity** — one fixed color per provider on every surface (widget limit rows, breakdown chips, learning badges, session-search badges): Claude `--provider-claude` orange `#fb923c`, Codex `--provider-codex` blue `#60a5fa`, MiniMax `--provider-minimax` violet, sub-agent `--provider-agent` orchid. Blue/orange is the colorblind-safe two-group pairing, deliberately redder than caution amber so identity never reuses a status hue; the `shared` learning scope renders neutral. In the Models view, individual models render as rank-assigned shades of their provider's family ramp.
- **Metric identity** — the six readout hues and the three context-category hues, confined to sparkline strokes, endpoints, swatches, and split-bar segments. A category that would otherwise want green/red (added versus removed lines) takes `--metric-net-lines` / `--metric-loc-per-hr` instead, because a category is not a threshold.
- **Signal cyan** (`--signal-cyan` `#22d3ee`) carries two roles that are both Quill speaking about itself: the brand mark and update invitation, and the runtime/throughput metric. It is never a provider hue and never interactive chrome.
- Memory type badges: blue (user), red (feedback), green (project), yellow (reference), purple (claude-md)

### Responsive Scaling

The widget main window resizes freely but its density does not scale with it, so the `--s` fit-to-height system and the per-layout `quill-size-*` sizes it depended on no longer have a consumer.

Type sizes, gutters, and the vertical ladder stay fixed at the 360px design width whatever the window measures.

Height behaves the same way: because the ladder is fixed, each view has one natural height at 360px wide rather than a range, which is what makes a static default honest. Usage is both the default and the tallest — 788px measured against the browser mock with its breakdown saturated at `BREAKDOWN_LIMIT` rows — while Trends, Charts, Models, and Context all measured shorter on the same fixtures, between 452px and 678px. A default sized for Usage therefore clears every view, and the shorter ones simply leave surface below their last band rather than scrolling.

Bands absorb the width instead of scaling into it. The shell is a flex column at `height: 100vh`, the content column has no width of its own, `.wg-grid` splits into three `1fr` tracks, and rows are flex lines whose text cells carry `min-width: 0` so they ellipsize rather than push. The one band with an intrinsic width wider than the 320px floor is `.wg-footer`, which wraps its Manage affordance onto a second line under that pressure; at the design width and above it stays a single 40px row. The `.wg-row` hover bleed (`margin: 0 -7px`) reports as horizontal overflow on `.wg-rows` at every width — it is contained by the band's 14px gutter and clipped by `.wg-scroll`, which is the intent.

Accessibility zoom is unaffected — [[src/main.tsx]] still applies Ctrl+`+`/`-`/`0` webview zoom per window, and the widget's content column scrolls so a zoomed-in layout stays reachable. No `--s` declaration survives in `src/styles/index.css`: the scaling system was deleted with the components that consumed it.

## Utilities

Shared formatting helpers under `src/utils/`. The chart helpers went with the charting library; the widget viz kit owns its own geometry.

| File | Exports |
|------|---------|
| `src/utils/format.ts` | `formatNumber()` (thousand separators), `formatDurationSecs()` (human-readable) |
| `src/utils/tokens.ts` | `formatTokenCount()` (1.2M, 5.4k display) |
| `src/utils/time.ts` | `timeAgo()` (ISO string to relative "5m ago") |
| `src/utils/providers.ts` | `providerLabel()`, `normalizeProviderScope()`, `providerScopeLabel()`, `providerFilterLabel()`, `providerBadgeClass()`, `providerScopeClass()`, `memoryTypeLabel()`, `PROVIDER_ASYMMETRY_DISCLOSURE` |
| `src/utils/retention.ts` | `retentionSpanFor()`, `isPruned()`, `formatRetentionCutoff()`, `PRUNED_PLACEHOLDER` — see [[frontend#Frontend#Components#Retention Degradation]] |
