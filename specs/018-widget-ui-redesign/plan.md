# Plan: widget-ui-redesign

## Architecture Approach

Big-bang replacement of the main window behind a stable data layer. The React
shell (`src/App.tsx` and everything it composes) is rewritten as a widget shell
— titlebar + LIMITS + a swappable view region — while the Rust/IPC layer is
*extended, never broken*: new range plumbing and three new aggregates land
first, existing commands keep their signatures where possible. Old UI files
are deleted in the same feature (constitution: no legacy), but only after the
new shell renders against browser fixtures, so the tree never passes through a
state where neither UI works.

Charting: Recharts is removed from the dependency tree. All widget
visualization (usage chart, six sparklines, compact Trends/Charts/Models
graphics) is served by a small internal SVG kit (`src/components/widget/viz/`)
implementing the mockup's exact rendering (catmull-rom line/area, endpoint
markers, hover legend, bar/heat primitives). Rationale: the mockup's
overlaid-headline + surface-fade + hover-chip treatments are hostile to
Recharts; the kit is ~300 lines vs a 3.7 MB dependency; and after the old
analytics views are deleted there is no other Recharts consumer (verified:
only `src/components/analytics/*` imports it).

Alternatives rejected:
- Navigation-only dropdown with views hosted in Manage — rejected at the
  clarify gate (Q1=B): views render compactly in-widget below LIMITS.
- Keeping Recharts for the compact views — two chart systems in one 360px
  bundle; mockup fidelity would still require custom SVG for the hero chart.
- Incremental migration (old and new UI behind a flag) — violates the "no
  legacy code" mandate and doubles the verification surface.

## Affected Components

**New (src/components/widget/):**
- `WidgetTitleBar.tsx` — brand mark, centered update button (visible only when
  the existing 4-hour `check()` finds an update; wired to
  `install_app_update`), sync freshness pill (real elapsed time; slate
  offline/paused/cached variants absorb today's `UsageDisplay` pill logic),
  always-on-top toggle (reads/writes persisted `always_on_top` via
  `set_runtime_settings`; tray + Settings stay in sync through the existing
  `runtime-settings-updated` event), settings key, close key (close-to-tray
  preserved), `data-tauri-drag-region`.
- `LimitsSection.tsx` — one row per enabled provider: swatch + name, one
  bucket cell per rate-limit window (percent, label, 4px bar, 50/80 severity
  classes, stale-bucket severity suppression carried over), right-aligned
  nearest-reset countdown; MiniMax setup/unavailable rendered with the
  existing status-pill vocabulary; section absent when no provider enabled.
- `ViewSwitcher.tsx` — the dropdown (listbox semantics, Escape/outside-click,
  focus-visible) swapping the view region.
- `views/UsageView.tsx` — usage band (range toggles 1H/6H/24H/7D, provider
  series chart with overlaid value+delta and hover legend chip), insight line
  (cache savings v1 from `useContextSavingsStats`), 3×2 readout grid
  (metric-hue swatches + sparklines), breakdown section (five modes), footer
  (In/Out/Cache + Manage ⌘M app-scoped accelerator).
- `views/TrendsView.tsx`, `views/ChartsView.tsx`, `views/ModelsView.tsx`,
  `views/ContextView.tsx` — compact 360px adaptations, same visual system,
  same underlying hooks as their predecessors; honesty disclosures (retention
  banner condensed line, Hooks asymmetry `?`) keep compact homes.
- `viz/` — Sparkline, AreaChart (multi-series + overlay slot + hover chip),
  Bars, Heat primitives.
- Widget styles live as new sections inside `src/styles/index.css` (one
  global stylesheet, constitution #2 — no separate widget.css file): Flat
  Polish tokens (`--surface #14181f`, hairlines, the six metric hues, and the
  provider hues `--provider-claude #fb923c` / `--provider-codex #60a5fa` /
  `--provider-minimax` reused from the existing stack) added to `:root`.
- Accessibility semantics (matching the app's existing conventions —
  `aria-pressed` button groups, `role="status"`): limit bars are
  `role="progressbar"` with `aria-valuenow/min/max`; the sync pill is
  `role="status"` `aria-live="polite"` (it absorbs the old pill region's
  semantics); range, breakdown, and any toggle strips are labeled button
  groups with `aria-pressed`; the view dropdown is a listbox with
  `aria-selected` on the active option; every interactive element has a
  visible focus state.
- Compact view content contracts (what each view shows at 360px — the
  fixture-first design pass refines layout, not scope):
  - **Trends:** three week-over-week rows (tokens, velocity, cache
    efficiency), each: metric name, this-week value, delta vs last week,
    paired mini-bars; condensed retention-degradation line when retention
    affects the compared windows.
  - **Charts:** the three synchronized series (tokens, code changes, cache
    efficiency) stacked as compact area/bar charts sharing one time axis and
    one hover crosshair; range follows the widget range toggles.
  - **Models:** running-now strip (provider badge + current model + since),
    then a session-ranked model list (swatch+model id, sessions bar, tokens);
    tapping a row is deferred (no inspect panel in v1 — full detail remains
    out of widget scope).
  - **Context:** headline preserved/retrieved totals, cache-savings line
    (same source as the insight line), and the routing-cost readout.
- Breakdown row fields per mode (designed here per US7 AC):
  - **Sessions:** status dot, project name, provider tag, token total,
    recency; header carries live count.
  - **Projects:** project name, session count, token total, last-active.
  - **Hosts:** hostname, token total, session count (volume-sorted).
  - **Skills:** skill name, uses count, last used (ALL-TIME scope as today).
  - **Hooks:** hook identity, event count, QUILL chip where Quill-deployed,
    header `?` tooltip carrying the Claude/Codex tracking-asymmetry
    disclosure; condensed retention line when degraded (constitution #1).

**Modified:**
- `src/App.tsx` — becomes the widget shell (~150 lines): provider/usage
  polling retained, update check retained, right-click Refresh/Quit retained,
  close-to-tray retained; split-pane, divider, `--s` fit-to-height system,
  arrow-key resize, `quill-size-*` persistence all deleted.
- `src/main.tsx` — keeps webview zoom (Ctrl+/−/0) and window routing; drops
  `WindowResizeHandles` mount; adds ⌘M/Ctrl+M app-scoped accelerator opening
  Manage (focus-existing via `WebviewWindow.getByLabel`).
- `src-tauri/tauri.conf.json` — main window: `width: 360`,
  `resizable: false`, min/max height bounds; `alwaysOnTop` stays `false` in
  conf (fresh-install default `true` applied through the runtime-settings
  seed, preserving existing users' stored choice).
- `src-tauri/src/lib.rs` — window-state plugin: exclude the SIZE flag for the
  main window (decided; the `widget_ui_v1` settings marker is used only to
  seed the fresh-install always-on-top default, not for state reset) so stale
  geometry can't override the widget; position persistence kept; tray AOT
  checkitem unchanged (single persisted source).
- `src/components/settings/GeneralTab.tsx` + `SettingsWindowView.tsx` —
  remove the four orphaned controls, the `Layout:` config-summary line, and
  the UI-prefs branch of Reset-to-defaults (removal only). ADD (required by
  the chrome contract, since `TitleBar.tsx` is deleted): a version row
  (`getVersion()`) — the app's only version display — with a "What's new"
  link that opens the `release-notes` window, preserving that window's sole
  entry point.
- `src/mocks/ipcFixtures.ts` — fixtures for the new shapes (provider series,
  activity series, 6H ranges) so the widget renders in browser mode for
  visual verification.

**Deleted (the legacy sweep):**
`UsageDisplay.tsx`, `components/live/*`, `TitleBar.tsx`,
`WindowResizeHandles.tsx`, `components/analytics/AnalyticsView.tsx` + all tab
components + Recharts wrappers (`UsageChart`, `TokenSparkline`,
`CodeSparkline`, `ModelActivityChart`, `ChartsTab`, …) after their compact
replacements land, `hooks/useUiPrefs.ts` + `ui-prefs-updated` emit/listen
sites, divider/split CSS + all `var(--s)` scaling blocks (dead-CSS removal is
grep-verified — knip does not see CSS), localStorage keys
(`quill-layout-mode`, `quill-time-mode`, `quill-show-live`,
`quill-show-analytics`, `quill-split-ratio{,-h}`, `quill-size-*`;
`quill-charts-range` silently dropped), npm deps `recharts` and `geist`
(unused). Hooks that feed views (`useTokenData`,
`useCodeInsights`, `useLlmRuntimeStats`, `useBreakdownData`,
`useContextSavingsStats`, `useSessionSubagents`, usage-data hooks) survive.

**Docs:** DESIGN.md + `.impeccable/design.json` rewritten (Flat Polish,
whole-app, amended color/type laws, migration note for non-widget windows;
the old "two densities" doctrine is replaced by a single flat density with a
stated exception that legacy windows keep their current density until their
own redesign pass);
lat.md — `frontend.md` (Main Window Layout, Responsive Scaling, Components),
`features.md` (Live Usage View, Analytics Dashboard, Cross-Window UI Sync,
Settings Tab Layout), `architecture.md` (Multi-Window Design), plus every
wiki link touching deleted files (~50 refs); `release_notes.md` entry
announcing the replacement and the preference reset; PRODUCT.md line citing
the shipped design updated (11px → new system).

## Data Model

No SQLite schema changes. New read-side aggregates only:

- `get_provider_token_series(range, buckets)` — single SQL over
  `token_snapshots` grouping by aligned time bucket × provider; returns
  per-provider aligned series + per-provider totals; the summed series equals
  `get_token_stats` for the same range by construction (same WHERE clause) —
  constitution #1.
- `get_activity_series(range, buckets)` — per-bucket distinct session count
  and distinct project count (sessions/projects sparklines).
- Range plumbing: `RangeType` gains `"6h"`; commands currently taking
  `days: i32` (`get_token_stats`, `get_session_breakdown`,
  `get_project_breakdown`, `get_host_breakdown`, `get_skill_breakdown`,
  `get_hook_breakdown`) gain hour-granular scoping via a shared
  `range: String` parameter (back-compat wrapper keeps `days` until the last
  frontend caller migrates inside this feature, then the wrapper is deleted).
  The `6h` arm must be added at EVERY range matcher — enumerated so none
  silently falls through to a 24h default (constitution #1):
  `range_to_duration` (`storage.rs:1279`), `get_token_history`
  (`storage.rs:9842`), `get_code_stats_history` `bucket_secs`
  (`storage.rs:16365`), every other `"1h" =>` match arm across
  `storage.rs`/`models.rs` (8 sites — grep-audited during implementation),
  `context_savings_from_timestamp`; TS mirrors: `RangeType` and `ModelRange`
  in `src/types.ts`, and the per-hook range→days maps in `useTokenData.ts`,
  `useAnalyticsData.ts`, `useCodeInsights.ts`. A grep for `"1h"` match arms
  with no `"6h"` sibling is part of the work item's acceptance.
- Per-metric sparkline sources (all from the selected range; 8 buckets):
  runtime → `LlmRuntimeStats.sparkline` (extended to honor 1H/6H);
  tok/LOC + LOC/hr → `useCodeInsights` buckets over `get_code_stats_history`
  (bucket count raised 7→8, 6h bucket arm added);
  net lines → per-bucket `lines_added − lines_removed` from
  `CodeStatsHistoryPoint`;
  sessions + projects → new `get_activity_series`;
  hero chart → new `get_provider_token_series`.
- Settings: new persisted marker `widget_ui_v1` (window-state reset guard);
  `always_on_top` seed logic for fresh installs.

## API / Interface Changes

- New Tauri commands: `get_provider_token_series`, `get_activity_series`.
- Changed: the six stats/breakdown commands accept range strings including
  `6h`/`1h` (breaking for the old `days` argument — all callers are updated
  in-feature; no external API surface exists).
- Removed events: `ui-prefs-updated`. Kept: `runtime-settings-updated`,
  committed-analytics refresh signals.
- UI surface: main window is the widget (breaking visual change; release
  note); Manage/settings/release-notes windows unchanged except orphan
  removal; ⌘M/Ctrl+M accelerator added (app-scoped — no global-shortcut
  plugin; avoids macOS minimize collision).

## Testing Strategy

Constitution #7: no new automated test code (not authorized). Verification is:
1. Existing gates zero-warning: `cargo fmt --check`, `cargo clippy`,
   `npm run lint`, `tsc --noEmit`, `npm run build`, `cargo build`, existing
   test suites unchanged and passing (constitution #6).
2. Visual pass: widget rendered in browser mode against `ipcFixtures`,
   headless-Chrome screenshots at 360px/1× compared against
   `specs/018-widget-ui-redesign/mockup.tpl.html` (fonts injected) for the
   Usage view; compact views reviewed against the Flat Polish checklist
   (hairlines-only, 8px floor, color law, tabular numerics). Cross-cutting
   checks on every view: no layout shift on value ticks (before/after
   screenshot diff while fixtures advance), reduced-motion honored on the
   sync pulse, dropdown, and legend chip, visible focus on every interactive
   element, and text contrast at or above the current app baseline.
3. State pass: fixture-driven checks of empty/loading/offline/paused/cached
   variants, no-provider (LIMITS absent), MiniMax setup row, update-button
   visibility, stale-bucket severity suppression.
4. Dead-code sweep: `knip` added as a devDependency with a scoped config;
   gate = zero *new* unused exports/files vs a baseline captured before the
   feature (pre-existing debt like `IntegrationsWindow.tsx` is recorded in
   the baseline and filed as a follow-up bead, not fixed here).
5. Perf budgets (constitution #10): cold widget first-paint ≤ 500 ms after
   webview ready (measured via performance.mark in dev build, 3-run median);
   steady-state refresh cycle main-thread ≤ 50 ms per 60 s tick (Chrome
   tracing); main-window JS bundle strictly smaller than before (Recharts
   removal; vite build output compared). Initial IPC staggered so first paint
   never waits on breakdown queries. If the refresh budget is missed, the
   named remediation is bounding `get_code_stats_history` (cache parsed
   `full_input` results per row id or LIMIT the scan window) before any
   budget relaxation is considered.
6. `lat check` passes after doc updates (constitution #8).

## Risks

- **Compact views are design work, not just code** (Trends/Charts/Models/
  Context at 360px were never mocked). Mitigation: each view bead starts with
  a fixture-rendered static pass reviewed against the Flat Polish system
  before wiring; Usage view's kit (viz/, tokens) is built first so the
  language is mechanical by the time the four views start.
- **Aggregate/headline divergence** (per-provider series vs totals).
  Mitigation: one SQL source per range for both, asserted equal in dev via
  debug logging; divergence is a bug by definition.
- **Window-state fights the fixed size on upgrade.** Mitigation: SIZE flag
  exclusion + `widget_ui_v1` marker; verified by launching with a seeded old
  state file.
- **Wayland/AOT no-op.** Mitigation: toggle reflects the OS result (typed
  failure surfaces as a disabled state with tooltip), never silently lies
  (constitution #5).
- **Unconditional analytics IPC regresses responsiveness.** Mitigation:
  staggered initial fetch, 60 s cadence unchanged, budget measured before
  merge (constitution #3/#10).
- **Big-bang App.tsx rewrite.** Mitigation: new shell developed side-by-side
  behind the fixtures until it renders all states, then the old tree is
  deleted in one commit within the feature — never shipped dual.
- **Rollback:** single squash commit on main; revert restores the old UI
  wholesale (no schema migrations to unwind).

## Sequencing

Ordered work items; letters are for this document's prose only (bead titles
stay descriptive, ordering lives in dependency edges):

1. **Design tokens + viz kit** — widget CSS tokens, Sparkline/AreaChart/Bars/
   Heat primitives, keycap/toggle/pill primitives, fixtures for new shapes.
   (Blocks everything UI.)
2. **Rust range + aggregates** — `6h` range, hour scoping across six
   commands, `get_provider_token_series`, `get_activity_series`; TS types +
   hooks updated. (Parallel with 1; blocks Usage view wiring.)
3. **Window shell** — tauri.conf geometry, window-state SIZE
   exclusion/reset marker, drag region, close-to-tray, right-click menu,
   ⌘M accelerator, AOT titlebar wiring + fresh-install default,
   WidgetTitleBar with update button + sync pill (incl. degraded variants).
   (Needs 1.)
4. **LIMITS section** — provider rows, severity, stale suppression, MiniMax
   states, no-provider absence. (Needs 1; parallel with 3.)
5. **Usage view** — chart + overlay + hover legend, range toggles, insight
   line, readout grid + sparklines, breakdown (Sessions rows + the four
   compact modes incl. disclosures), footer. (Needs 1+2; 3/4 for full
   assembly.)
6. **Compact Trends view** / 7. **Compact Charts view** /
   8. **Compact Models view** / 9. **Compact Context view** — one bead each,
   fixture-first design pass then wiring; ViewSwitcher integration.
   (Each needs 1+2; parallel with each other after 5 establishes the
   language.)
10. **Legacy teardown** — delete old components/hooks/CSS/keys/deps, settings
    orphan removal, `--s` removal, knip baseline + sweep. (Needs 3–9.)
11. **Docs + design system** — DESIGN.md + sidecar rewrite, lat.md updates
    across all touched sections/links, release-notes entry, PRODUCT.md line.
    (Needs 10 for final truthfulness; drafting can start after 5.)
12. **Verification + budgets** — quality gates, visual pass, state pass,
    perf measurements, `lat check`. (Last; blocks feature close.)
13. **Follow-up beads (created, not executed here):** marketing-site +
    README screenshot refresh + capture-script rework (script clicks deleted
    titlebar buttons at fixed offsets); repo-wide dead-code sweep of
    pre-existing debt (baseline from knip); insight-line rotation rule;
    compact-view polish round after real-data soak.

## Backlog Refinement

None — no P4 sources exist in scope (spec Backlog Inputs: None). Related
non-P4 bead `quill-qwx` (P1 design tracker) is superseded by the new epic at
create-beads with a link so its decision notes stay reachable
(disposition recorded there; it is not a backlog-refinement input).

## Target Epic

No existing epic; create-beads creates the feature epic for
widget-ui-redesign and supersedes `quill-qwx` with it.

## Constitution Check

1. Local source-backed truth — new aggregates share WHERE clauses with
   headline stats; no invented series; disclosures keep compact homes. ✓
2. Established stack — Rust/Tauri IPC extended, React strict TS, pure CSS
   tokens; no new frameworks (Recharts *removed*). ✓
3. Responsive execution — aggregates are read-side SQL; staggered fetch; no
   UI-thread I/O. ✓
4. Recoverable mutation — settings writes go through existing runtime-settings
   path; window-state reset is marker-guarded and one-time. ✓
5. Typed failure boundaries — degraded states surface as typed slate pill
   variants; AOT failure surfaces, never swallowed. ✓
6. Zero-warning gates — enumerated in Testing Strategy; knip added as a
   scoped gate with baseline. ✓
7. Authorized behavior testing — no new test code; verification is manual +
   existing suites. ✓
8. Architecture traceability — lat.md work item 11 + `lat check` in 12. ✓
9. Glass Cockpit discipline — DESIGN.md is rewritten *by this feature* (the
   clarified Q7 wording); until item 11 lands, the old doc and the new UI
   disagree — tension is explicit and resolved within the feature. ✓(noted)
10. Measured performance — explicit budgets + methods in Testing Strategy. ✓
11. Explicit external transmission — none added. ✓
12. Gated delivery — beads DAG, worktree, squash commit; no push. ✓

## Alignment fixes applied

- (A, must) Release-notes window kept reachable: settings window gains a
  version row + "What's new" link, replacing deleted `TitleBar.tsx` as the
  sole entry point; version display homed in settings per clarify Q5.
- (A, must) 6h range plumbing enumerated site-by-site (`range_to_duration`,
  `bucket_secs`, all `"1h" =>` arms, `context_savings_from_timestamp`,
  `ModelRange` + hook day-maps) with a grep acceptance check so no matcher
  silently defaults to 24h.
- (A, must) Per-metric sparkline data sources named for all six metrics.
- (A, must) `quill-show-live` / `quill-show-analytics` added to the
  localStorage deletion list.
- (A, must) Accessibility semantics specified (progressbar bars, status sync
  pill, aria-pressed toggle groups, listbox dropdown with aria-selected).
- (A, must) Breakdown row fields designed per mode in the plan (US7 AC), and
  compact view content contracts added for Trends/Charts/Models/Context.
- (A/B, should) CSS file contradiction resolved — all styles live in
  `src/styles/index.css`; provider-hue tokens added to the token work item.
- (A/B, should) Window-state either/or resolved: SIZE-flag exclusion decided;
  marker only seeds fresh-install AOT default.
- (A, should) Visual pass extended with layout-shift, reduced-motion,
  focus-visible, and contrast checks; per-band empty/loading states assigned
  to the token/primitive work item via view content contracts.
- (A, should) Perf remediation path named for `get_code_stats_history`.
- (A, should) Dead-CSS verification method stated (grep; knip is TS-only).
- (A, should) DESIGN.md rewrite now states the fate of the "two densities"
  doctrine.
- (B, should) README screenshots added to the follow-up screenshot bead.
- Note: subagent B (plan quality) terminated early on a session limit; its
  pass was completed by the orchestrator inline — findings folded above.
