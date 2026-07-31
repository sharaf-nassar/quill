# Spec: widget-ui-redesign

## Problem Statement

Quill's main window is a split-pane Live/Analytics dashboard that the team has
rejected: it splits attention across two competing panes, duplicates controls,
and over-weights subscription rate limits — which are an optional integration —
while burying the analytics and insights that are the actual product. Seventeen
mockup iterations (Proposal 17, "Flat Polish") converged on a replacement: the
main window becomes a single 360px always-on-top monitoring widget, usage-first,
with limits compressed to a compact per-provider section. The old UI is to be
removed entirely — no legacy layout, no dead components, no unused styles,
hooks, or settings — and the governing design documents (DESIGN.md,
`.impeccable/design.json`, lat.md) updated so the new system is the recorded
intent. The users are Quill's operators: developers running Claude Code and
Codex who keep the widget glanceable in a screen corner mid-session.

## Goals

- The main window renders the Proposal 17 widget (as amended by
  Clarifications): titlebar (brand mark, centered update button when an update
  is available, sync freshness pulse, always-on-top toggle, settings entry,
  close button), LIMITS section (one provider per row, per-bucket utilization
  bars with 50/80 severity thresholds, per-provider reset countdown), and a
  **view region below LIMITS** whose content is swapped by the view-switcher
  dropdown. The default Usage view fills it with: usage band (dropdown,
  centered 1H/6H/24H/7D range toggles, provider-series token chart with
  value+delta overlaid top-left and hover-only legend chip carrying
  per-provider totals), one computed insight line, 3×2 metric readout grid
  (runtime, tok/LOC, LOC/hr, sessions, projects, net lines — each with
  metric-hue swatch and sparkline), breakdown section
  (Sessions/Projects/Hosts/Skills/Hooks toggles, entity rows, live count),
  and footer (In/Out/Cache totals, Manage ⌘M entry). Selecting Trends,
  Charts, Models, or Context replaces everything below LIMITS with a compact
  360px adaptation of that view in the same visual system. There is no pace
  annunciator — severity lives on the limit bars alone.
- `specs/018-widget-ui-redesign/mockup.tpl.html` is the visual source of truth;
  the shipped widget must be visually faithful to it at 1× on a 360px window.
- All old main-window UI is deleted: split-pane layout, draggable divider,
  Live/Analytics pane components, their CSS, hooks, localStorage preferences,
  and settings toggles that configured them. `Zero` unused exports/styles
  remain from the old main window after the change (verified by a dead-code
  sweep, not by hand-waving).
- DESIGN.md and `.impeccable/design.json` are rewritten to the Flat Polish
  system (flat surface `#14181f` on the existing graphite stack, hairline
  dividers only, no boxes/gradients/glow, 9px type floor, severity
  green/amber/red reserved, fixed provider hues, six metric hues restricted to
  sparklines+swatches, signal-cyan brand). lat.md sections covering the main
  window, Live Usage View, and Analytics Dashboard are updated and `lat check`
  passes.
- Existing quality gates (fmt, lint, typecheck, build) pass with zero warnings.
- Bead `quill-qwx` (design-phase tracker) is superseded by the new feature epic.

## Non-Goals

- Direct API-cost tracking (explicitly future work; the design leaves room).
- MiniMax feature work beyond keeping its existing enable/setup state rendering
  correctly in the new limits section when enabled.
- Redesigning the Manage workspace, settings window, release-notes window, or
  the marketing site (they stay on their current design until a later pass).
  Amendment (clarify gate): removing the Settings→General controls orphaned by
  deleted prefs (layout, time visualization, show-Live/show-Analytics, and the
  config-summary/Reset-to-defaults references to them) IS in scope — removal
  only, no settings redesign. Marketing-site/README screenshot refresh and the
  capture-script rework are follow-up beads created by this feature, not part
  of it.
- New analytics computations beyond what the widget displays (the insight line
  and sparklines draw on existing data; any genuinely new aggregate is scoped
  in Open Questions, not silently invented — constitution #1).
- Expanding automated test coverage (constitution #7: requires explicit user
  authorization; none given yet).
- Onboarding/first-run flow redesign.

## Backlog Inputs

None. No epic was supplied; the hierarchy/provenance closure is empty, and no
open P4 sources exist for this feature. Related non-P4 context: `quill-qwx`
(P1 feature, open) tracked the mockup/design phase and carries decision notes;
it is input context and will be superseded by this feature's epic.

## Target Epic

No existing epic. This run creates the feature epic during create-beads;
`quill-qwx` will be linked (supersede or discovered-from) so its decision
history stays reachable.

## User Stories

1. As an operator mid-coding-session, I want a compact always-on-top widget
   showing my token usage, key metrics, and live sessions, so that I can stay
   ahead of what my agents are burning without leaving my editor.
   - AC: main window opens as a 360px-wide always-on-top widget rendering all
     Proposal 17 bands with live data; no split-pane, divider, or second pane
     exists anywhere in the main window code path.
   - AC: every live number uses tabular figures and updates without layout
     shift.
   - AC: the widget honors `prefers-reduced-motion` (sync pulse, dropdown and
     legend transitions).

2. As an operator, I want window chrome that carries the app lifecycle, so
   nothing I rely on today disappears.
   - AC: titlebar shows brand mark left; a centered update button appears only
     when an update is available (wired to the existing 4-hour update check
     and `install_app_update`); right side carries sync freshness, the
     always-on-top toggle (reading/writing the existing persisted
     `always_on_top` setting so tray and Settings stay in sync), settings
     entry, and a close button (close-to-tray behavior preserved). The window
     remains draggable (drag region) and the right-click Refresh/Quit menu
     survives. Version display lives only in the settings window.
   - AC: there is no pace/annunciator band; severity colors follow the 50/80
     thresholds on the limit bars; degraded data states (offline, paused,
     cached) surface as slate variants of the titlebar sync pill, never as
     red banners; each band defines one empty/loading state.

3. As an operator with one or more provider subscriptions connected, I want a
   compact per-provider limits readout, so that limits are checkable but never
   dominate.
   - AC: one row per enabled provider (swatch + name + one bar per bucket with
     percent and label + right-aligned nearest reset countdown); rows align
     into a scannable table.
   - AC: with no provider connected, the LIMITS section is absent entirely and
     the rest of the widget renders unaffected.
   - AC: provider identity hues are fixed (Claude orange, Codex blue, MiniMax
     violet) and never collide with severity colors.

4. As an operator, I want the usage chart headline overlaid on the chart with
   per-provider series, so the widget stays short.
   - AC: total + delta render over the chart's top-left with the
     surface-colored fade treatment from the mockup; series stay legible
     behind it.
   - AC: hovering (or scrubbing, on the real chart) reveals the legend chip
     with per-provider totals; it is hidden at rest.
   - AC: range toggles (1H/6H/24H/7D) re-scope the chart, headline, delta,
     insight, and metric sparklines together.

5. As an operator, I want to switch the analytics view from the widget, so
   deeper views are one click away.
   - AC: the view dropdown lists Usage, Trends, Charts, Models, Context with
     the active view checked; keyboard focus and Escape behave as in the
     mockup.
   - AC: selecting a view replaces everything below the LIMITS section with a
     compact 360px in-widget adaptation of that view (titlebar and LIMITS
     persist across views); the old full-window Trends/Charts/Models/Context
     implementations are deleted, not kept behind navigation.

6. As an operator, I want the six key metrics distinguishable at a glance,
   so the readout grid reads instantly.
   - AC: each metric carries its fixed hue (runtime cyan, tok/LOC violet,
     LOC/hr magenta, sessions indigo, projects teal, net lines lime) on
     exactly its label swatch, sparkline, and endpoint — values stay white.
   - AC: sparklines are computed from the selected range's real series.

7. As an operator, I want the breakdown section switchable between Sessions,
   Projects, Hosts, Skills, and Hooks, so the widget answers "what was I just
   doing" the way the old analytics pane did.
   - AC: toggle strip matches the mockup; Sessions is default; session rows
     show status dot, name, provider tag, token total, recency; live count
     shown.
   - AC: all five modes render compact in-widget rows for their entity type;
     row fields per mode are designed in plan following the Flat Polish
     system; required honesty disclosures (retention degradation, Hooks
     tracking asymmetry) keep a compact home in their modes (constitution #1).

8. As an operator, I want the footer to show In/Out/Cache totals and open the
   Manage window with ⌘M, so everything editable lives outside the widget.
   - AC: footer totals track the selected range; the Manage button and the
     global ⌘M shortcut open the existing Manage workspace; the widget itself
     contains no editable settings.

9. As a maintainer, I want the legacy main-window UI gone and the design docs
   truthful, so the codebase carries no dead weight.
   - AC: split-pane components, divider logic, old pane CSS, related hooks,
     localStorage keys (layout/panel visibility/time-mode for the old panes),
     and settings toggles for the old layout are deleted; a dead-code check
     shows no unused exports introduced or left behind by this feature.
   - AC: DESIGN.md + `.impeccable/design.json` describe the shipped system;
     lat.md updated; `lat check` passes; constitution #8/#9 satisfied.

## Constraints

- Visual source of truth: `specs/018-widget-ui-redesign/mockup.tpl.html`
  (copied from the working mockup; Geist/Geist Mono injected from
  `src/assets/fonts/*.woff2`). Published reference:
  https://claude.ai/code/artifact/b4f873e0-5721-4f18-8ced-8028be669526
- Stack: existing Rust/Tauri IPC + React 19 strict TypeScript, pure CSS design
  tokens in `src/styles/index.css` (constitution #2). New tokens extend the
  existing `:root` graphite stack; the widget surface is `#14181f`.
- Color law: severity green/amber/red reserved for threshold state; provider
  hues fixed; metric hues (cyan `#22d3ee`, violet `#a78bfa`, magenta
  `#f472b6`, indigo `#818cf8`, teal `#2dd4bf`, lime `#a3e635`) appear only on
  metric swatches/sparklines/endpoints.
- Typography: Geist variable (already wired via `@font-face` in
  `src/styles/index.css`); 8px minimum size (mockup wins over the earlier 9px
  wording); tabular numerics on all live values.
- Window geometry (clarified): fixed 360px width, `resizable: false`,
  content-driven height with sane min/max; `tauri_plugin_window_state` saved
  geometry reset on first run of the new UI; window position persistence kept;
  webview zoom (Ctrl+/−/0) kept; the `--s` fit-to-height scaling system
  deleted.
- Data contract (clarified): new Rust work is in scope — 6H range across the
  range enums, hour-granular scoping for stats/breakdown commands, a
  provider-keyed bucketed token-series aggregate whose sum matches the
  headline total, and per-metric sparkline series for sessions and projects.
  Nothing is trimmed from the design.
- The widget stays responsive: no DB/network work on UI threads (constitution
  #3); data arrives via existing polling/IPC paths.
- Keyboard focus visible on all interactive elements; contrast per current
  baseline; reduced-motion respected (constitution #9 as amended by the new
  DESIGN.md).
- Commit/track via Beads; quality gates zero-warning before completion
  (constitution #6, #12).

## Open Questions

1. **Non-Usage views in the dropdown:** do Trends/Charts/Models/Context render
   as compact in-widget views (large adaptation effort per view), or does
   selecting them open the full view in the Manage/analytics window (small
   effort, dropdown acts as navigation)? Mockup only demonstrates the menu.
2. **Window geometry:** is the widget fixed at 360px width? Fixed height or
   content-driven (annunciator/limits appear and disappear)? Is user resizing
   retained at all (the old UI had keyboard zoom and persisted sizes via the
   `--s` scaling variable)?
3. **Old analytics components:** Trends/Charts/Models/Context views currently
   live in the main window. If Q1 resolves to "open full view elsewhere",
   where do they live (Manage workspace tab?) and do their implementations
   survive unchanged behind that navigation?
4. **Chart engine:** the mockup uses hand-rolled SVG; the app uses Recharts.
   Rebuild the widget chart/sparklines as lightweight SVG (drop Recharts from
   the main window bundle) or keep Recharts? Bundle size vs. effort.
5. **Insight line source:** cache-savings copy exists in context-savings stats
   today. Is the insight line fixed to cache savings v1, or a rotating set
   (needs a selection rule — constitution #1 forbids invented insights)?
6. **Net lines + per-metric sparkline series:** which existing IPC queries
   serve 8-point series per metric per range, and does "net lines" need a new
   aggregate or reuse of added/removed totals?
7. **localStorage migration:** delete old keys silently on first run of the
   new UI, or one-time migrate anything (e.g., chart range preference)?
8. **MiniMax row:** when MiniMax is enabled but in setup/unavailable state,
   what does its limits row show (the old UI had setup/unavailable states)?
9. **Offline/cached states:** the old Live pane had "Offline — showing cached
   data" and "Paused" pills. Where do these surface in the widget (titlebar
   sync area? annunciator variant?) — nominal/disconnected/empty states were
   never mocked.
10. **Fixed 3s sync cadence display:** freshness shows seconds-ago; old data
    refresh was 3-minute polling. Does the widget change any polling cadence,
    or only presentation?
11. **quill-qwx disposition:** supersede with the new epic (proposed) or keep
    open as the epic itself?

## Clarifications

**Q1: View routing — in-widget compact views or navigation to full views?**
A: In-widget (option B). The dropdown replaces everything below the LIMITS
section with a compact 360px adaptation of the selected view
(Usage/Trends/Charts/Models/Context). Titlebar and LIMITS persist across
views. The old full-window analytics implementations are deleted — no legacy
kept behind navigation. All five breakdown modes likewise render compact
in-widget rows. (Reflected in Goals, US5, US7. Open Questions 1/3 resolved.)

**Q2: Data contract vs. design trim?**
A: Accept the Rust work; trim nothing. (Reflected in Constraints. Open
Question 6 resolved; OQ4's chart-engine choice moves to plan with the note
that Recharts can only leave the main-window bundle entirely since the full
views are deleted.)

**Q3: Window geometry & persistence?**
A: Accepted as recommended: fixed 360px width, content-driven height,
window-state reset on first run, position persistence kept, webview zoom
kept, `--s` deleted. (Reflected in Constraints. Open Question 2 resolved.)

**Q4: Pace-alert formula + degraded states?**
A: The "ahead of pace" concept is removed entirely — no annunciator band, no
pace description anywhere. Severity is carried by the limit bars alone.
Degraded data states (offline/paused/cached) surface as slate variants of the
titlebar sync pill; each band defines one empty/loading state. (Reflected in
Goals and US2. Open Questions 9/10 resolved: freshness label shows real
elapsed time; polling cadence unchanged.)

**Q5: Widget chrome contract?**
A: Update button appears centered in the titlebar only when an update is
available; version display stays in the settings window only. Remainder as
recommended: close button kept (close-to-tray preserved), right-click
Refresh/Quit and drag region kept, always-on-top toggle in the titlebar
reads/writes the existing persisted setting (single source of truth with tray
and Settings), default flips to true for fresh installs only. (Reflected in
Goals and US2.)

**Q6: Settings & cross-window teardown scope?**
A: Accepted as recommended: orphaned Settings→General controls removed
(removal only), `useUiPrefs`/`ui-prefs-updated` deleted, marketing-site and
screenshot-tooling fallout filed as follow-up beads. (Reflected in Non-Goals.
Open Question 7 resolved: silent-delete localStorage keys.)

**Q7: Design-law wording for the DESIGN.md rewrite?**
A: Accepted as recommended: mockup wins; severity hues never carry other
meanings (trend/status greens allowed); no decorative gradients (functional
surface-fade allowed); 8px floor; cyan dual-role (brand + runtime metric)
acknowledged; DESIGN.md rewritten as the whole-app system with non-widget
windows conforming at their next touch. (Reflected in Constraints. Open
Questions 5/8/11 resolved per Non-Blocking Observations: insight v1 = cache
savings; MiniMax row reuses status pill vocabulary; quill-qwx superseded by
the new epic.)

## Spec Review

### Critical Questions (answer before planning)

1. **View routing (the estimate-defining fork):** does selecting
   Trends/Charts/Models/Context in the widget dropdown (a) render compact
   in-widget adaptations (~2,800 lines of tab code re-laid-out at 360px — four
   separate design efforts), or (b) navigate to the full views hosted
   elsewhere — which means the Manage workspace gains an Analytics section,
   colliding with the "settings/Manage untouched" Non-Goal? Same fork applies
   to non-Sessions breakdown modes (Projects/Hosts/Skills/Hooks carry filter
   strips, sort headers, drilldowns, and honesty disclosures that cannot fit a
   360px row without redesign). Recommendation: (b) navigation-only v1,
   Sessions-only breakdown v1 with the other four toggles present but
   navigating/deferred; compact adaptations become phase-2 beads.
   — flagged by: requirements, feasibility, scope, stakeholders

2. **Data contract vs. design trim:** the current data layer cannot serve the
   mockup: no 6H range anywhere (`RangeType` is 1h/24h/7d/30d), all
   totals/breakdowns take whole `days: i32` (1H/6H scoping impossible),
   `TokenDataPoint` has no provider dimension (per-provider chart series with
   a matching headline sum needs a new Rust bucketed aggregate), and
   session-count/project-count sparkline series don't exist (constitution #1
   forbids fabricating them). Accept new Rust aggregates + range plumbing as
   in-scope, or trim the design (drop 6H, drop those two sparklines)?
   Recommendation: accept the Rust work — it is the honest version of the
   design; drop nothing.
   — flagged by: requirements, feasibility, scope

3. **Window geometry & persistence:** fixed 360px (resizable: false) or not?
   Fixed height or content-driven (annunciator/LIMITS appear/disappear)?
   `tauri_plugin_window_state` restores old geometry (280×340 conf default,
   520×700 persisted "both") over any new size — migration/reset needed.
   Window *position* memory for a corner widget is absent from the spec
   entirely. Fate of keyboard zoom (`quill-zoom-main`, the app's stated
   accessibility mitigation) and the `--s` fit-to-height system must be
   stated. Recommendation: fixed width 360, content-driven height with
   min/max, reset window-state on first run of the new UI, keep position
   persistence, keep webview zoom, delete `--s`.
   — flagged by: requirements, gaps, feasibility, scope, stakeholders

4. **Alert semantics + degraded states:** "ahead of pace" has no formula
   (comparison, margin, hysteresis, multi-bucket behavior, amber vs red band).
   And the widget has no designed home for today's contractual degraded
   states: "Offline — showing cached data", "Paused" (401), "Showing cached
   data" (429), stale-bucket severity suppression, provider-empty/first-run/
   loading states — upgrading users will hit these on first launch.
   Recommendation: pace = utilization% > elapsed%-of-window + 5pt margin with
   exit hysteresis; degraded states surface as slate variants of the
   annunciator band + titlebar sync pill; empty states get one designed
   pattern per band.
   — flagged by: requirements, gaps, ambiguity, scope, stakeholders

5. **Titlebar & lifecycle surface losses:** the mocked titlebar drops the
   Update pill (the app's only update affordance), the version/release-notes
   entry, and the close button; right-click Refresh/Quit and the drag-region
   for the decorationless window are unspecified; always-on-top would have
   three owners (tray checkitem, Settings toggle, new titlebar button) with
   no source-of-truth rule and an implied default flip from `false`. Decide
   the widget's full chrome contract. Recommendation: keep close button;
   version+update fold into the settings/tray path or an annunciator-style
   update line; titlebar AOT toggle reads/writes the existing persisted
   `always_on_top` key (tray/settings stay in sync via the existing event);
   default remains user's stored value, `true` only for fresh installs.
   — flagged by: gaps, scope, stakeholders

6. **Settings & cross-window teardown scope:** deleting the old prefs guts
   Settings→General (four controls + config summary line + Reset-to-defaults
   branch) and empties `useUiPrefs`/`ui-prefs-updated` — but Non-Goals
   declares the settings window out of scope. Amend the Non-Goal to permit
   removal of orphaned controls (no redesign), and state the disposition of
   the hook/event and the marketing-site/screenshot-tooling fallout
   (README + marketing screenshots show the deleted UI; capture script
   clicks deleted buttons) as explicit follow-up beads.
   — flagged by: gaps, scope, stakeholders

7. **Design-law vs. mockup contradictions (resolve before DESIGN.md rewrite):**
   the spec says 9px type floor but the mockup ships 8/8.5px runs; says
   severity colors "appear nowhere else" but the mockup uses green for sync
   pulse/live dots/delta-up and red for delta-down; says "no gradients" but
   requires the surface-fade headline treatment; assigns cyan to both brand
   and the runtime metric. Also: does the rewritten DESIGN.md govern only the
   widget (other windows stay on the old system — then the doc must say so)
   and what happens to the old "two densities" doctrine? Recommendation:
   mockup wins; restate laws as "severity hues never mean anything else"
   (trend/status greens allowed), "no decorative gradients" (functional fade
   allowed), floor = 8px, cyan = brand + runtime metric acknowledged; rewrite
   DESIGN.md as whole-app system with an explicit migration note that
   non-widget windows conform at their next touch.
   — flagged by: ambiguity, feasibility, scope, stakeholders

### Non-Blocking Observations

- **Dead-code goal needs tooling + scope trim:** name the tool (knip or
  ts-prune as a dev-dependency gate) and scope the AC to "old-main-window
  artifacts deleted; no *new* unused exports" — a repo-wide zero-dead-code
  gate would block this feature on pre-existing debt (e.g.
  `IntegrationsWindow.tsx`). File the global sweep as its own bead.
- **Performance budgets (constitution #10):** the widget makes analytics IPC
  unconditional for the first time (`get_code_stats_history` re-parses JSON
  per row). Plan must set explicit budgets (first-paint, per-refresh CPU,
  bundle delta) and a measurement method.
- **⌘M:** no global-shortcut plugin exists and OS-global ⌘M collides with
  macOS minimize; recommend app-scoped accelerator only.
- **Insight line v1:** pin to cache-savings from `useContextSavingsStats`;
  rotation rule is a follow-up.
- **localStorage:** silent-delete the enumerated keys (`quill-layout-mode`,
  `quill-time-mode`, `quill-split-ratio{,-h}`, `quill-size-*`); only
  `quill-charts-range` is even arguably worth migrating — and the range set
  changed anyway.
- **MiniMax limits row:** reuse the existing ON/SETUP/UNAVAILABLE pill
  vocabulary compressed into the row; resolve in plan.
- **Freshness label:** presentation-only; must show real elapsed time
  (polling is 3-min/60s), never imply a 3s cadence.
- **Retention/hooks honesty disclosures:** `RetentionBanner` and the Hooks
  asymmetry `?` must have stated homes if their surfaces appear in the widget
  (constitution #1) — v1 scope depends on Q1's answer.
- **Verification ownership:** visual ACs (fidelity, tabular figures, no
  layout shift, reduced motion) close via a named manual pass (headless
  screenshot comparison against the mockup at 360px), since new automated
  tests are unauthorized (constitution #7).
- **Accessibility semantics:** annunciator as `role="status"` live region,
  `progressbar` semantics on utilization bars, toggle strips as labeled
  button groups with `aria-pressed` (matching the existing app convention).
- **Geist is already wired** (`@font-face` in `src/styles/index.css`); the
  `geist` npm package is an unused dep — delete as part of cleanup.
- **quill-qwx:** supersede with the new epic.
