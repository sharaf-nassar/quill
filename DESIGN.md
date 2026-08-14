---
name: Quill
description: A calm, exact instrument for AI coding agents — one flat plane, ruled by hairlines.
colors:
  surface: "#14181f"
  inset: "#0f1319"
  menu-raised: "#1b212b"
  console-black: "#121216"
  panel-deep: "#0d1117"
  panel-raised: "#1e1e24"
  card-graphite: "#161b22"
  slate-input: "#1a1a1f"
  graphite-line: "#21262d"
  line: "#ffffff10"
  line-soft: "#ffffff0b"
  hover: "#ffffff0a"
  text-hi: "#e6edf3"
  text: "#c9d1d9"
  readout: "#d4d4d4"
  label: "#8b949e"
  faint: "#6e7681"
  meter-green: "#34d399"
  meter-amber: "#fbbf24"
  meter-red: "#f87171"
  signal-blue: "#60a5fa"
  signal-cyan: "#22d3ee"
  signal-violet: "#a78bfa"
  signal-orchid: "#c084fc"
  provider-claude: "#fb923c"
  provider-codex: "#60a5fa"
  provider-minimax: "#a78bfa"
  provider-agent: "#c084fc"
  metric-runtime: "#22d3ee"
  metric-tok-per-loc: "#a78bfa"
  metric-loc-per-hr: "#f472b6"
  metric-sessions: "#818cf8"
  metric-projects: "#2dd4bf"
  metric-net-lines: "#a3e635"
  context-preserved: "#22d3ee"
  context-retrieved: "#60a5fa"
  context-routing: "#a78bfa"
typography:
  hero:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "26px"
    fontWeight: 650
    lineHeight: 1
    letterSpacing: "-0.02em"
    fontFeature: "'tnum' 1"
  value:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "15px"
    fontWeight: 600
    lineHeight: 1.15
    letterSpacing: "-0.01em"
    fontFeature: "'tnum' 1"
  row:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "11.5px"
    fontWeight: 500
    lineHeight: 1.3
  body:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "10.5px"
    fontWeight: 500
    lineHeight: 1.45
  meta:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "10px"
    fontWeight: 500
    lineHeight: 1.2
  label:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "10px"
    fontWeight: 600
    lineHeight: 1
    letterSpacing: "0.1em"
  micro:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "8px"
    fontWeight: 500
    lineHeight: 1.2
  mono:
    fontFamily: "'Geist Mono', ui-monospace, SFMono-Regular, Menlo, monospace"
    fontSize: "10px"
    fontWeight: 500
    lineHeight: 1.4
    fontFeature: "'tnum' 1, 'zero' 1"
rounded:
  sharp: "0"
  xs: "2px"
  sm: "4px"
  md: "6px"
  lg: "8px"
  shell: "12px"
  pill: "999px"
spacing:
  "2xs": "2px"
  xs: "4px"
  sm: "6px"
  md: "8px"
  lg: "10px"
  xl: "12px"
  "2xl": "16px"
  gutter: "14px"
components:
  key:
    backgroundColor: "transparent"
    textColor: "{colors.faint}"
    rounded: "{rounded.md}"
    padding: "0"
    typography: "{typography.meta}"
  toggle-on:
    backgroundColor: "#ffffff14"
    textColor: "{colors.text-hi}"
    rounded: "{rounded.md}"
    padding: "3px 9px"
    typography: "{typography.meta}"
  toggle-off:
    backgroundColor: "transparent"
    textColor: "{colors.faint}"
    rounded: "{rounded.md}"
    padding: "3px 9px"
    typography: "{typography.meta}"
  update-invite:
    backgroundColor: "#22d3ee1a"
    textColor: "{colors.signal-cyan}"
    rounded: "{rounded.md}"
    padding: "0 10px"
    typography: "{typography.meta}"
  pill-status:
    backgroundColor: "transparent"
    textColor: "{colors.faint}"
    rounded: "{rounded.sharp}"
    padding: "0"
    typography: "{typography.meta}"
  bar-track:
    backgroundColor: "#ffffff14"
    textColor: "{colors.text}"
    rounded: "{rounded.pill}"
    padding: "0"
  rule:
    backgroundColor: "{colors.line-soft}"
    textColor: "{colors.faint}"
    rounded: "{rounded.sharp}"
    padding: "0"
  listbox:
    backgroundColor: "{colors.menu-raised}"
    textColor: "{colors.text}"
    rounded: "{rounded.lg}"
    padding: "4px"
    typography: "{typography.meta}"
  badge-provider:
    backgroundColor: "#ffffff0a"
    textColor: "{colors.provider-codex}"
    rounded: "{rounded.pill}"
    padding: "1px 6px"
    typography: "{typography.micro}"
  input-search:
    backgroundColor: "{colors.slate-input}"
    textColor: "{colors.readout}"
    rounded: "{rounded.md}"
    padding: "8px 10px"
    typography: "{typography.body}"
---

# Design System: Quill

## 1. Overview

**Creative North Star: "Flat Polish"**

Quill is an instrument, not a dashboard. The whole app is **one flat plane** —
a single near-black surface, ruled into bands by hairlines, carrying nothing
that is not a number, a label, or the shape of a number. There are no cards, no
raised panels, no gradients, no glow. Depth is spent only on things that
genuinely leave the plane: a dropdown, a tooltip, a dialog. Everything else is
separated by a 1px line and 12–16px of quiet.

This is the discipline of a glass cockpit taken to its conclusion. A cockpit
tiers its alarms by severity and refuses to decorate; Flat Polish keeps the
severity meter, the tabular figures, and the strict color budget, and removes
the last of the chrome — the boxes. Quill watches the one thing a coding agent
cannot see about itself (what it is burning and where it stands against the
limits) and reports it at a glance, from a 360px widget that lives in a screen
corner while the operator works.

The system runs at **one density.** The always-on-top widget is the primary
surface and sets the rhythm at its 360px design width: a 14px gutter,
8/10/12/16px vertical spacing, a 10px base type size, hairline separators
between bands. The window is freely resizable on both axes, so 360px is the
width the density is tuned for, not a width the layout may assume — bands stay
fluid and every one of them must read from a 320px minimum up to a full-screen
drag. Everything Quill ships is meant to converge on that rhythm. See
[§6 Density and Migration](#6-density-and-migration) for the one stated
exception — the legacy Manage/settings/release-notes windows, which keep their
current roomier density until their own redesign pass.

It explicitly rejects the four houses of generic dark UI. It is **not** a
generic SaaS template (rounded cards, gradient hero, pill buttons, big-number
panels). It is **not** AI-hype or crypto (neon gradients, glassmorphism, glow).
It is **not** a playful consumer app (bubbly palettes, heavy rounding, emoji,
mascots). It is **not** corporate enterprise (stock-photo blue, gray-on-gray,
marketing fluff). Instrument-grade sits in the narrow band between
over-alerting and under-informing: dense, quiet, semantic.

**Key Characteristics:**
- One flat surface (`#14181f`) for the whole widget; bands are separated by
  hairlines, never by boxes.
- A reserved three-color severity meter (green / amber / red) that is the
  system's spine and carries no other meaning anywhere.
- Color is identity or severity — never decoration. Chrome is grayscale.
- Numbers are tabular and never reflow; values are bright, labels are dim.
- Type runs 8px → 26px, with 8px as a hard floor.
- A single density; motion is short, functional, and reduced-motion aware.

## 2. Colors

One flat plane needs fewer surfaces and stricter hues. The palette is a
surface pair, a hairline ladder, a brightness-only text ladder, and four
closed sets of meaningful color: severity, provider identity, metric identity,
and limit-window identity.

### Surfaces
- **Surface** (`#14181f`): the plane. The widget shell, every band, every row.
  There is no second resting surface — a band is not a lighter rectangle, it is
  the same surface between two hairlines.
- **Inset** (`#0f1319`): the only recess — chart grounds and bar tracks where
  a value must read *into* the plane.
- **Menu Raised** (`#1b212b`): the single lifted layer (the view listbox and
  any popover). It is the only surface allowed a shadow.
- **Line** (`#ffffff10`) / **Line Soft** (`#ffffff0b`): band separators and
  in-band rules. **Hover** (`#ffffff0a`): the only hover fill in the system.
- The legacy Graphite Stack (`--console-black`, `--panel-deep`,
  `--panel-raised`, `--card-graphite`, `--slate-input`, `--graphite-line`)
  survives for the not-yet-migrated windows only; see §6.

### The Text Ladder
Hierarchy is brightness, never hue.
- **Text Hi** (`#e6edf3`): values, headlines, the active option. The digits you
  came to read.
- **Text** (`#c9d1d9`): rows and running text.
- **Label** (`#8b949e`) / **Faint** (`#6e7681`): labels, units, meta, inactive
  controls, and every axis.

### Severity — The Meter
The instrument's spine. These three encode threshold state and **nothing
else.**
- **Meter Green** (`#34d399`): healthy — utilization below 50%; a delta whose
  direction is good.
- **Caution Amber** (`#fbbf24`): warning — utilization 50–80%; an actionable
  setup state.
- **Master-Warning Red** (`#f87171`): danger — utilization at or above 80%;
  error; destructive intent.

A degraded read is not an alarm: an unavailable provider, an offline poll, a
paused token, and a stale bucket all render **slate** (`--faint`), never red.
Red means a threshold was crossed, not that a request failed.

### Provider Identity
Category hues for telling agents apart, kept clear of the severity ramp so a
provider can never masquerade as a status.
- **Claude Orange** (`#fb923c`) — deliberately redder than caution amber.
- **Codex Blue** (`#60a5fa`) — blue/orange is the canonical colorblind-safe
  two-group pairing.
- **MiniMax Violet** (`#a78bfa`), **Agent Orchid** (`#c084fc`) — additional
  provider families and sub-agent/orchestration rows.

**The Model-Shade Rule.** A model is a shade of its provider's family ramp
(Claude orange family, Codex blue family, every other provider violet),
assigned by in-scope rank within the provider; rank seven and beyond folds to
neutral. Identity is always rendered swatch + raw id — a shade never stands
alone — and the same model keeps the same shade on every surface of a view.

### Metric Identity
Six fixed hues that name the six readouts, plus three that name the context
categories. Within metric readouts, they are permitted on **sparkline strokes,
their endpoint dots, label swatches, and split-bar segments only.** Values stay
Text Hi. Limit-window labels may reuse metric hues as category identifiers under
the rule below.
- Runtime `#22d3ee` · Tokens-per-LOC `#a78bfa` · LOC-per-hour `#f472b6` ·
  Sessions `#818cf8` · Projects `#2dd4bf` · Net lines `#a3e635`.
- Context: preserved `#22d3ee` · retrieved `#60a5fa` · routing `#a78bfa`.

### Limit Window Identity
Rate-limit labels reuse three established metric-category hues so adjacent
windows scan as distinct categories without borrowing the severity meter.
- **5-hour** uses Runtime light blue `#22d3ee` (`--metric-runtime`).
- **7-day** uses Projects teal `#2dd4bf` (`--metric-projects`).
- **Fable** uses Tokens-per-LOC purple `#a78bfa`
  (`--metric-tok-per-loc`).

The color is confined to the raw text label; label wording and cell position
remain the primary cues, and every other dynamic window stays neutral.

### Named Rules

**The Severity Code Rule (amended).** Green, amber, and red are reserved for
threshold state. They never decorate, never brand, never indicate category,
and they never mark a failed or degraded read — that is slate. A delta may take
green or red only when its *meaning* is known to be good or bad (a falling
tokens-per-LOC is an improvement and reads green); a delta whose goodness is
unknown stays neutral. If a green thing is not "healthy," it is a bug.

**The Reserved-Status Rule.** Provider, metric, and limit-window identity never
use the severity tokens, in either direction. Where a diverging pair is
genuinely a category rather than a threshold (added versus removed lines), it
is drawn from the metric ramp (`--metric-net-lines` up,
`--metric-loc-per-hr` down), never from green/red.

**The Cyan Category Rule.** Signal Cyan `#22d3ee` is Quill's own brand accent —
the update invitation, the marketing tie — and the Runtime metric hue. The
5-hour limit label explicitly reuses the Runtime category token. These roles
cannot be confused with a provider or severity; that is exactly why cyan is
never assigned to a provider family. Cyan is not an interactive accent:
selection and focus remain Signal Blue.

**The Dimming Ladder Rule.** Hierarchy is built by brightness, not hue. Step
down Text Hi → Text → Label → Faint, or a ladder of white alpha for chrome.
Reach for a new color only when it carries new meaning.

**The No-Decorative-Gradient Rule.** Gradients are permitted in exactly one
place: the vertical fade under a chart area, where it is the series' own
value-encoding surface. Nowhere else — no gradient backgrounds, buttons,
borders, headers, or text. Every other fill is flat.

## 3. Typography

**Display / Data / Body Font:** Geist (with `-apple-system`, Segoe UI fallback)
**Mono Font:** Geist Mono (with `ui-monospace`, SF Mono fallback)

**Character:** Geist is built for data and developer surfaces — neutral, sharp,
with real tabular figures. Both faces are self-hosted variable fonts, so the app
and the marketing site speak one voice. One family in many weights carries the
entire instrument; there is no display/body pairing.

### Hierarchy
- **Hero** (650, 26px, tracking −0.02em, tabular): the one headline value
  overlaid on the usage chart. Exactly one per view.
- **Value** (600, 15px, tracking −0.01em, tabular): readout-cell and trend
  values — the numbers in the grid.
- **Row** (500, 11.5px): entity names and their primary values in list rows.
- **Body** (500, 10.5px): running sentences — the insight line, empty-state
  copy, disclosures.
- **Meta** (500/600, 10px): the working size of the instrument — labels,
  toggles, the sync pill, the view switcher, chips.
- **Micro** (500, 8px): the floor — rate-limit window labels and chart axis
  ticks.
- **Mono** (Geist Mono, 500, 10px, tabular + slashed-zero): raw model ids,
  session ids, paths, and any aligned identifier column.

### Named Rules

**The 8px Floor Rule.** No text renders below 8px. 8px is reserved for
non-essential orientation labels (a window name already stated in the bar's
accessible name, an axis tick); anything a user must read to act is 10px or
larger. Below 8px the instrument stops being legible and starts being texture.

**The Tabular Rule.** Every live or comparative number uses tabular figures
(`font-variant-numeric: tabular-nums`). A readout that reflows as its value
ticks is broken. Non-negotiable on meters, values, deltas, countdowns, and any
numeric column.

**The Mono-for-Truth Rule.** Monospace is for things that *are* code or
identifiers — raw model ids, session ids, paths. It is never used on a label
"to look technical." If you only want digits to stop jittering, that is
`tabular-nums`, not a monospace font.

## 4. Surface and Elevation

Quill is flat. A band, a row, a readout cell, a chart — none of them is a
box: no fill of its own, no border, no shadow, no radius. They are regions of
the one surface, separated by a 1px `line-soft` rule and by space. The only
radii in the system belong to the window shell (12px), controls (4–8px), and
tracks (pill).

Depth is spent on exactly one thing: a layer that has genuinely left the plane.

### Shadow Vocabulary
- **Listbox / Popover** (`box-shadow: 0 8px 24px rgba(0,0,0,0.6)`): the view
  switcher's menu and any tooltip. The only everyday shadow.
- **Modal** (`box-shadow: 0 24px 40px rgba(0,0,0,0.45)`): confirmation dialogs
  in the management windows — the rare full-attention layer.

### Named Rules

**The Floats-Only Rule.** A shadow means "this floats." A region of the plane
never has one. If a band looks like it needs a card, it needs a hairline and
more space instead. Glow, neon, and colored shadows are forbidden — they are
the AI-hype tell.

**The Hairline Rule.** Structure is carried by 1px lines at 4–6% white and by
the spacing ladder. Two adjacent bands get one rule between them, never a rule
each; a rule inside a band is `line-soft`, a rule between bands is `line`.

## 5. Components

Every interactive element carries default, hover, focus-visible, and (where it
applies) pressed and disabled states. Focus is the global keyline:
`outline: 2px solid rgba(96,165,250,0.7); outline-offset: 2px`. Controls are
transparent at rest and reveal themselves on hover with the single `hover`
fill — the plane stays quiet until you reach for it.

### Keys (icon controls)
- **Shape:** 24×24 grid cell, 6px radius, transparent ground, `faint` glyph.
- **States:** hover lifts the glyph to Text Hi over the `hover` fill; a
  latched key (always-on-top engaged) keeps the brighter glyph. 120ms on
  color and background; never animate layout.
- This is the whole titlebar right cluster: always-on-top, settings, close.

### Toggle Strips (range, breakdown mode, any button group)
- **Shape:** 6px radius, 3px × 9px padding, 10px meta type, transparent at rest.
- **Selected** is `aria-pressed="true"` and *only* that: an 8% white fill with
  Text Hi at weight 600. Selection is never a hue — a colored pressed state
  would compete with severity.
- These are labeled button groups, not tablists; the accessible state and the
  visual state are the same attribute.

### The View Switcher
- A **listbox**, because the control has a value: `aria-haspopup="listbox"`
  with `aria-expanded` on the trigger, exactly one `aria-selected` option,
  keyboard movement via `aria-activedescendant`.
- The trigger is 10px uppercase label type with a chevron that rotates 180° on
  open (150ms, suppressed under reduced motion). The menu is the system's one
  raised surface: `menu-raised` ground, `line` border, 8px radius, popover
  shadow.

### Status Pills and Chips
- **Sync control:** the native button at the right of the LIMITS header has a
  text-and-hairline-dot readout, no capsule, tabular elapsed time in `faint`,
  and an `aria-busy` state during a live request. Its degraded variants
  (offline, paused, cached) key off `data-state` and stay slate — it never
  turns red.
- **Identity chip:** 999px, 8px micro type, 1px×6px, a ~10% tint of the
  provider's fixed hue with the hue at full strength as text. One provider, one
  color, every surface.
- **Lamp states:** a provider with no live data states `SETUP` in amber when
  the failure is actionable and `UNAVAILABLE` in slate otherwise — a word and a
  color, no box.

### The Update Invitation
The app's only update affordance, centered in the titlebar and rendered only
once the check has found a release: brand cyan text on a 10% cyan wash with a
30% cyan hairline, 6px radius. It is an invitation, not an alarm — which is
precisely why it is cyan and not amber.

### Signature Component — The Limit Cell
The clearest expression of the North Star. Per rate-limit window: a compact
header places its 8px window label at inline-start and the rounded percent (12px
on provider summaries, 11px on account rows, tabular) geometrically centered
over the track. Below it, a 4px `pill`-radius track on an 8% white ground has a
discrete `green | amber | red` class chosen by the 50/80 thresholds. The fill
transitions width at 0.3s ease — meter ballistics, calm rather than twitchy.
The track is a real `role="progressbar"` with `aria-valuenow/min/max` and the
untruncated window label as its accessible name. A bucket whose reset has
already elapsed is `stale`: neutral slate, no severity, because a utilization
measured against a bygone window is not a live threshold. An optional CPA reset
sits centered on a dedicated footer beneath that same track. Its 10px tabular
countdown uses the brighter secondary label tone so it stays legible without
competing with the utilization plane above.

Cells divide the available meter region evenly. Direct cells hold a 60px
legibility floor; CPA cells hold 88px so utilization, window, and reset remain
associated before dynamic extras reflow. CPA cells wrap as equal flex lines:
multi-cell lines share the width and any lone wrapped cell fills its line.
Provider and disclosed account rows repeat the same 70px identity column and
meter layout, making their values scan vertically from the 320px floor through
a full-screen drag.

### Charts
- Drawn by an internal SVG kit, not a charting library. Primitives are
  Sparkline and AreaChart (multi-series with an overlay slot and a hover-only
  legend chip).
- A sparkline is a metric-hue stroke at 60% opacity plus a solid endpoint dot:
  no axes, no ticks, no grid.
- An area series is kept inside the lower ~62% of its box so the overlaid
  headline never collides with the data, and its fill is the one permitted
  gradient.
- Every bar track is a real `role="progressbar"` with a formatted
  `aria-valuetext`; every chart that carries values a user must read exposes
  them to assistive tech rather than leaving them as pixel width.
- Gaps stay gaps. A bucket with no measurable value breaks the line; it is
  never drawn as zero.

### Inputs (management windows)
- **Style:** `slate-input` ground, 1px hairline, 6px radius, `readout` text,
  placeholder at 30% white.
- **Focus:** border shifts to Signal Blue with a matching 1px ring. **Focus is
  blue, not green** — a green focus ring collides with the severity meter and
  is prohibited.

### Motion
- 120ms on control color/background; 150ms on the chevron; 0.3s ease on bar
  and meter fills; 200ms `cubic-bezier(0.32,0.72,0,1)` for the one expressive
  reveal.
- Every animation, pulse, and skeleton shimmer is wrapped in
  `prefers-reduced-motion: no-preference`. Under reduced motion fills snap to
  value and nothing pulses.

## 6. Density and Migration

Flat Polish replaces the previous "two densities" doctrine (a dense Primary
Flight Display beside roomier Systems Pages) with **one density**, set at the
widget's 360px design width: a 14px gutter, an 8/10/12/16px vertical ladder,
10px base type, and hairline band separators. The density does not scale with
the window — a widget dragged to 1200px keeps the same gutter and type ladder
and simply gives its bands more room.

**The stated exception.** The Manage workspace (Sessions, Learning, Instances,
Settings), the settings surfaces, and the release-notes window still render in
their pre-existing roomier density on the Graphite Stack — cards, borders, and
the older 11px/9px type ladder. That is a migration state, not a second
doctrine: each of those windows keeps its current density until its own
redesign pass moves it onto the flat plane. Until then:

- Do **not** propagate card-and-border patterns into new surfaces. New work is
  built flat.
- Do **not** back-port Flat Polish tokens piecemeal into a legacy window; a
  window converts in one pass or not at all, so no surface is ever half-flat.
- The laws in §2 (severity reserved, provider identity fixed, no decorative
  gradients, focus is blue) apply to **every** window today, including the
  unmigrated ones. Only the surface treatment and spacing are deferred.

## 7. Do's and Don'ts

### Do:
- **Do** build on one flat surface and separate bands with a hairline and
  space.
- **Do** reserve green / amber / red for threshold state on the 50% / 80%
  thresholds, and render degraded or failed reads in slate.
- **Do** give every provider exactly one fixed family hue (Claude orange,
  Codex blue, MiniMax violet, Agent orchid) and shade models within their
  provider's family by in-scope rank.
- **Do** distinguish 5-hour, 7-day, and Fable limit labels with Runtime light
  blue, Projects teal, and Tokens-per-LOC purple while keeping their raw text
  and position as non-color cues.
- **Do** keep metric-readout hues on sparkline strokes, endpoints, and swatches
  only — values stay Text Hi. The named limit-window label reuse is the sole
  text exception.
- **Do** set `font-variant-numeric: tabular-nums` on every live or compared
  number, and keep type at 8px or above.
- **Do** make values bright and labels dim; build hierarchy with brightness and
  weight, not new hues.
- **Do** keep motion functional and fast (120ms on state, 0.3s on fills) and
  honor `prefers-reduced-motion`.
- **Do** expose the value: progressbar roles, live regions, and labeled button
  groups, so nothing has to be inferred from pixel width.

### Don't:
- **Don't** introduce a card, a panel border, or a second resting surface. If a
  band needs separating, it needs a rule and more space.
- **Don't** use a gradient anywhere except a chart area's own surface fade, and
  never a shadow on anything that has not left the plane.
- **Don't** ship a generic SaaS template: no gradient hero, no big-number
  metric panels, no pill buttons, no identical icon-heading-text card grids.
- **Don't** reach for AI-hype / crypto finishes: no glassmorphism, no glow, no
  colored shadows.
- **Don't** go playful-consumer: no bubbly palette, no corner radius above 12px
  (the shell) or 8px (anything inside it), no emoji, no mascots.
- **Don't** let a provider or metric hue borrow a status color, or let a status
  color mark a failed read. Claude is orange everywhere or it's broken.
- **Don't** color a selected state; selection is an 8% white fill plus
  brightness, and focus is `signal-blue`.
- **Don't** render text below 8px, or use proportional figures for live
  numbers.
- **Don't** put editable controls in the widget — search, forms, and settings
  belong in the management windows.
