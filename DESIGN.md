---
name: Quill
description: A calm, exact instrument for AI coding agents — the Glass Cockpit.
colors:
  console-black: "#121216"
  panel-deep: "#0d1117"
  panel-raised: "#1e1e24"
  card-graphite: "#161b22"
  slate-input: "#1a1a1f"
  graphite-line: "#21262d"
  hairline: "#ffffff1a"
  hairline-faint: "#ffffff0f"
  fill-ghost: "#ffffff0a"
  fill-hover: "#ffffff14"
  readout: "#d4d4d4"
  readout-bright: "#e6edf3"
  label: "#8b949e"
  label-faint: "#6e7681"
  meter-green: "#34d399"
  meter-amber: "#fbbf24"
  meter-red: "#f87171"
  signal-blue: "#60a5fa"
  signal-cyan: "#22d3ee"
  signal-violet: "#a78bfa"
  signal-orchid: "#c084fc"
  provider-claude: "#fb923c"
  provider-codex: "#60a5fa"
typography:
  data:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "20px"
    fontWeight: 700
    lineHeight: 1
    letterSpacing: "-0.01em"
    fontFeature: "'tnum' 1"
  title:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "13px"
    fontWeight: 600
    lineHeight: 1.3
  body:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "11px"
    fontWeight: 500
    lineHeight: 1.45
  label:
    fontFamily: "Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif"
    fontSize: "9px"
    fontWeight: 700
    lineHeight: 1
    letterSpacing: "0.11em"
  mono:
    fontFamily: "'Geist Mono', ui-monospace, SFMono-Regular, Menlo, monospace"
    fontSize: "11px"
    fontWeight: 500
    lineHeight: 1.45
    fontFeature: "'tnum' 1, 'zero' 1"
rounded:
  sharp: "0"
  xs: "2px"
  sm: "4px"
  md: "6px"
  lg: "8px"
  pill: "999px"
spacing:
  "2xs": "2px"
  xs: "4px"
  sm: "6px"
  md: "8px"
  lg: "10px"
  xl: "12px"
  "2xl": "16px"
components:
  button-primary:
    backgroundColor: "{colors.signal-blue}"
    textColor: "#0a0f1a"
    rounded: "{rounded.xs}"
    padding: "4px 9px"
    typography: "{typography.label}"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.label}"
    rounded: "{rounded.xs}"
    padding: "4px 11px"
    typography: "{typography.label}"
  toggle-on:
    backgroundColor: "#34d3991f"
    textColor: "{colors.meter-green}"
    rounded: "{rounded.sm}"
    padding: "4px 10px"
    typography: "{typography.label}"
  toggle-off:
    backgroundColor: "{colors.fill-ghost}"
    textColor: "{colors.label}"
    rounded: "{rounded.sm}"
    padding: "4px 10px"
    typography: "{typography.label}"
  range-tab-active:
    backgroundColor: "{colors.fill-hover}"
    textColor: "{colors.readout}"
    rounded: "{rounded.sm}"
    padding: "4px 10px"
  badge-provider:
    backgroundColor: "{colors.fill-ghost}"
    textColor: "{colors.provider-codex}"
    rounded: "{rounded.pill}"
    padding: "1px 6px"
    typography: "{typography.label}"
  input-search:
    backgroundColor: "{colors.slate-input}"
    textColor: "{colors.readout}"
    rounded: "{rounded.md}"
    padding: "8px 10px"
    typography: "{typography.body}"
  card:
    backgroundColor: "{colors.card-graphite}"
    textColor: "{colors.readout}"
    rounded: "{rounded.lg}"
    padding: "10px"
  tab-active:
    backgroundColor: "transparent"
    textColor: "{colors.readout-bright}"
    padding: "5px 14px"
---

# Design System: Quill

## 1. Overview

**Creative North Star: "The Glass Cockpit"**

Quill is an instrument, not a dashboard. A glass cockpit is a calm, dark panel that
surfaces system state and tiers its alarms by severity, so the operator stays ahead
of the aircraft without staring. Quill watches the one thing a coding agent can't see
about itself — what it is burning and where it stands against the limits — and reports
it with the exactness of a flight instrument. The aesthetic is borrowed from the
principles of that world (a strict color budget, severity tiers, numbers that never
jitter, hairlines instead of chrome), not from its skin. There are no rivets, no
gauges-as-decoration, no aviation cosplay.

The system runs in **two densities of the same instrument.** The **Primary Flight
Display (PFD)** — the always-on-top live analytics — is maximally dense: 11px type,
hairline dividers, tabular numerics, semantic meters you read at a glance while you
fly. The **Systems Pages** — session search, learning, plugins, agents, memory,
settings — are the same instrument zoomed out: roomier spacing, larger hit targets,
visible labels, one task per view. Same tokens, same control vocabulary, inverted
spacing. The PFD is for monitoring; the Systems Pages are for managing. Nothing
editable belongs in the PFD, and nothing glanceable needs the Systems Pages.

It explicitly rejects the four houses of generic dark UI. It is **not** a generic SaaS
template (rounded cards, gradient hero, pill buttons, big-number panels). It is **not**
AI-hype or crypto (neon gradients, glassmorphism, glow). It is **not** a playful
consumer app (bubbly palettes, heavy rounding, emoji, mascots). It is **not** corporate
enterprise (stock-photo blue, gray-on-gray, marketing fluff). Instrument-grade sits in
the narrow band between over-alerting and under-informing: dense, quiet, semantic.

**Key Characteristics:**
- Near-black canvas, chosen for contrast headroom — black gives the most room for
  high-contrast semantic signal.
- A reserved three-color severity meter (green / amber / red) that is the system's spine.
- Color is semantic-only and on a strict budget; chrome is grayscale.
- Numbers are tabular and never reflow; values are bright, labels are dim.
- Structure is carried by 1px hairlines and flat tonal panels — never by cards and shadows.
- Two densities, one identity: a dense cockpit and roomy systems pages.

## 2. Colors

A near-black instrument with a disciplined semantic vocabulary: grayscale chrome,
a reserved traffic-light meter, and a small ramp of cool hues for category identity.

### Primary
- **Signal Blue** (`#60a5fa`): the live, interactive hue. Selection, focus, primary
  actions, the active state of any control, the current-flight-display accent. The
  Codex provider family is based on this same blue; identity uses are always
  disambiguated from selection chrome by riding a badge or swatch with a name.

### Secondary — The Severity Meter
The instrument's spine. These three encode threshold state and **nothing else.**
- **Meter Green** (`#34d399`): healthy. Utilization below 50%; success; trend up.
- **Caution Amber** (`#fbbf24`): warning. Utilization 50–80%; needs-setup state; the
  update-available control.
- **Master-Warning Red** (`#f87171`): danger. Utilization at or above 80%; error;
  unavailable provider; destructive intent.

### Tertiary — Provider Identity
Category hues for telling agents apart: two maximally separated provider families
plus violet, kept clear of the severity ramp so a provider can never masquerade
as a status.
- **Claude Orange** (`#fb923c`): the Claude provider family. Deliberately redder
  than caution amber `#fbbf24` — amber remains severity-only.
- **Codex Blue** (`#60a5fa`): the Codex provider family. Blue/orange is the
  canonical colorblind-safe two-group pairing.
- **MiniMax Violet** (`#a78bfa`): the MiniMax provider (and any additional
  provider family); doubles as the secondary data-series color in charts.
- **Agent Orchid** (`#c084fc`): sub-agents and orchestration rows.
- **Signal Cyan** (`#22d3ee`): Quill's brand accent tying the app to the
  marketing surface — no longer a provider identity.

**The Model-Shade Rule.** A model is a shade of its provider's family ramp
(Claude `#fb923c → #7c2d12`/`#ffedd5`, Codex `#60a5fa → #16308f`/`#a7cdfd`,
others violet), assigned by in-scope rank within the provider; rank seven and
beyond folds to neutral. Identity is always rendered swatch + name — a shade
never stands alone — and the chart adjacency palette is validated for contrast
against the canvas. The same model keeps the same shade on every surface of a
page.

### Neutral — The Graphite Stack
- **Console Black** (`#121216`): the canvas. Body, titlebar, the PFD ground.
- **Panel Deep** (`#0d1117`): the deepest recess — tooltips, inset wells.
- **Panel Raised** (`#1e1e24`): floating layers — menus, popovers, context menus.
- **Card Graphite** (`#161b22`) + **Graphite Line** (`#21262d`): resting card surface
  and its 1px border. The card is defined by its border, not a shadow.
- **Slate Input** (`#1a1a1f`): form-field ground.
- **Readout** (`#d4d4d4`) / **Readout Bright** (`#e6edf3`): primary text and active
  headings — the digits you read.
- **Label** (`#8b949e`) / **Label Faint** (`#6e7681`): muted labels and secondary meta.

### Named Rules
**The Severity Code Rule.** Green, amber, and red are reserved for threshold state.
They never decorate, never brand, never indicate category. If a green thing is not
"healthy," it is a bug.

**The Reserved-Status Rule.** Provider identity (Claude orange / Codex blue /
MiniMax violet / Agent orchid) never overlaps the severity meter. Claude orange
is deliberately redder than caution amber `#fbbf24`, which stays severity-only;
a provider hue rendering as green or amber is forbidden — the exact drift this
system was built to kill (Claude once rendered blue, green, *and* purple across
three surfaces).

**The Dimming Ladder Rule.** Hierarchy is built by brightness, not hue. Step down a
ladder of white alpha (`rgba(255,255,255, .92 → .55 → .40 → .25)`) for chrome, or
between Readout → Label → Label-Faint for content. Reach for a new color only when it
carries new meaning.

## 3. Typography

**Display / Data Font:** Geist (with `-apple-system`, Segoe UI fallback)
**Body & Label Font:** Geist (same family, lower weights)
**Mono Font:** Geist Mono (with `ui-monospace`, SF Mono fallback)

**Character:** Geist is Vercel's typeface built for data and developer surfaces — neutral,
sharp, with real tabular figures. It is already self-hosted in Quill's brand, so the app
and the marketing site finally speak one voice. The family is listed first with the
system stack behind it, so the instrument degrades gracefully to system fonts until Geist
is wired into the app. One family in many weights carries the entire instrument; there is
no display/body pairing — product UI doesn't need one.

### Hierarchy
- **Data** (Geist, 700, 20px, line-height 1, tabular): the big live readouts — token
  totals, the headline number on an insight card. The instrument's largest type.
- **Title** (Geist, 600, 13px): section and card headings, panel names.
- **Body** (Geist, 500, 11px, line-height 1.45): rows, primary text, the base size.
- **Label** (Geist, 700, 9px, uppercase, letter-spacing 0.11em): meta, badges, the
  small all-caps tags on controls. The dense cockpit's connective tissue.
- **Mono** (Geist Mono, 500, 11px, tabular + slashed-zero): session ids, file paths,
  code, diffs, and any aligned numeric column.

### Named Rules
**The Tabular Rule.** Every live or comparative number uses tabular figures
(`font-variant-numeric: tabular-nums`). A readout that reflows as its value ticks is
broken. This is non-negotiable on meters, stat values, and any numeric table column.

**The Mono-for-Truth Rule.** Monospace is for things that *are* code or identifiers —
ids, paths, diffs, log lines. It is never used on a label "to look technical." If you
only want digits to stop jittering, that is `tabular-nums`, not a monospace font.

## 4. Elevation

Quill is flat at rest and lifts only what floats. A resting surface — a card, a row, a
panel — is defined by a flat graphite fill and a 1px hairline or graphite-line border,
never by a shadow. Depth appears exactly when a layer leaves the plane of the canvas:
menus, tooltips, and popovers cast a soft, dark, diffuse shadow that says "this is
above the instrument." Because the ground is near-black, these shadows are dark rather
than gray, and they carry no colored glow.

### Shadow Vocabulary
- **Menu** (`box-shadow: 0 4px 12px rgba(0,0,0,0.5)`): dropdown and context menus
  raised off the canvas.
- **Popover** (`box-shadow: 0 8px 24px rgba(0,0,0,0.6)`): tooltips and detached
  floating panels — the highest everyday layer.
- **Modal** (`box-shadow: 0 24px 40px rgba(0,0,0,0.45)`): confirmation dialogs, the
  rare full-attention layer.
- **Inset Ring** (`box-shadow: inset 0 0 0 1px rgba(255,255,255,0.20)`): a 1px keyline
  for keyboard focus on chrome where an outline would clip.

### Named Rules
**The Floats-Only Rule.** A shadow means "this floats." A surface sitting on the canvas
never has one. If a card has a drop shadow, delete the shadow and give it a
`graphite-line` border instead. Glow, neon, and colored shadows are forbidden — they
are the AI-hype tell.

## 5. Components

Every interactive component carries default, hover, focus-visible, and (where it
applies) active and disabled states. Focus falls back to the global keyline:
`outline: 2px solid rgba(96,165,250,0.7); outline-offset: 2px`. Density is the variable
between modes — the PFD packs these tight; the Systems Pages give them room — but the
shape, color logic, and states never change between the two.

### Buttons
- **Shape:** square to barely-softened (2px). Pills are reserved for status badges, not buttons.
- **Ghost (default control — the feature tabs):** transparent ground, `label`-dim text,
  9px uppercase with wide tracking. Hover lifts text toward `readout-bright`; the active
  tab brightens and grows a 1.5px underline that wipes in (200ms,
  `cubic-bezier(0.32,0.72,0,1)`). This is the cockpit's primary navigation gesture.
- **Primary (committed action):** filled `signal-blue` with near-black (`#0a0f1a`) text —
  the one place blue becomes a background. Used sparingly, for the active filter or the
  one affirmative action in a view.
- **Hover / Focus:** 0.15s ease on color and background; never animate layout.

### Toggles (provider / feature switches)
- **Shape:** 4px radius, 48px min-width, 9–10px uppercase label, tabular.
- **States map to the severity vocabulary:** ON = `meter-green` text on a 12%-green
  fill; OFF = `label`-dim on `fill-ghost`; SETUP = `caution-amber`; UNAVAILABLE =
  `meter-red`; BUSY = `signal-blue` with a slow 1.1s opacity pulse. The switch *is* the
  status light.

### Range & Tab Controls
- **Range tabs (1H / 24H / 7D / 30D):** a segmented group on a `fill-ghost` ground, 6px
  radius, 2px inset. The active tab fills to `fill-hover` with `readout` text; the
  group dims to 35% opacity when a selection elsewhere overrides it.
- **Analytics tabs (Now / Trends / Charts / Models / Context):** underline
  indicator, never a pill. A 2px bottom border is transparent by default and
  appears when active. Models uses `signal-blue` as interactive selection chrome;
  this accent never colors raw model identifiers or implies model-family identity.
  Provider badges retain fixed provider hues, model IDs remain neutral, and the
  five-tab layout scrolls horizontally without truncating labels. Active labels
  brighten to `readout-bright`.
- **Model history focus:** the selected-model chart series uses `signal-blue` and
  a visible "Selected model" label because blue means current selection. Every
  selected model uses that same treatment; provider badges alone carry provider
  identity, and no raw model ID receives a generated hue.
- **Range/provider filter semantics:** use separately labeled native-button groups
  with `aria-pressed`; these are filters, not nested tablists. Chart buckets expose
  the same bounds and series values in a visually hidden semantic table.

### Chips & Badges
- **Provider badge:** a 999px pill, 9px uppercase, 1px 6px. Background is a ~10% tint of
  the provider's identity hue; text is the hue at full strength. The fixed code —
  Claude orange, Codex blue, MiniMax violet, Agent orchid — is law (see The
  Reserved-Status Rule). One provider, one color, every surface.
- **Outlined glyph chip (e.g. ∞ ALL TIME):** transparent with a 1px `hairline` border,
  2px radius, 9px uppercase. Active inverts to a filled `signal-blue` with dark text.
- **Lifecycle badge:** 3px radius, 10px uppercase, a 12%-alpha tint of its state hue.
  States borrow the severity logic (confirmed=green, stale=amber, rejected=red,
  candidate=violet, retired=gray).

### Cards / Containers
- **Corner Style:** 8px (`rounded.lg`).
- **Background / Border:** `card-graphite` fill with a 1px `graphite-line` border.
- **Shadow Strategy:** none. Cards rest on the canvas (see The Floats-Only Rule).
- **Internal Padding:** 10px in the PFD; step up to 12–16px in the Systems Pages.
- Cards are containers of last resort. Prefer a hairline-divided list to a grid of
  cards; never nest a card inside a card.

### Inputs / Fields
- **Style:** `slate-input` ground, 1px `hairline` border, 6px radius (4px for compact
  selects), `readout` text, placeholder at 30% white.
- **Focus:** border shifts to `signal-blue` with a matching 1px ring
  (`box-shadow: 0 0 0 1px rgba(96,165,250,0.25)`). **Focus is blue, not green** — a
  green focus ring collides with the severity meter and is prohibited.

### Signature Component — The Usage Meter Row
The hero of the PFD and the clearest expression of the North Star. A label/value/countdown
top line over a thin track. The track is `fill-ghost`; the fill is a discrete
`meter-green | amber | red` class chosen by the 50/80 thresholds. A thin pace-marker
(a 2px tick) shows where you *should* be against elapsed time, so utilization reads
against pace, not in a vacuum. The percent text uses a continuous green→amber→red
gradient interpolation so the number itself carries severity. Fills transition width at
0.3s ease — meter ballistics, calm rather than twitchy. Tabular figures throughout.

## 6. Do's and Don'ts

### Do:
- **Do** reserve green / amber / red for status only, on the 50% / 80% thresholds. The
  meter is the spine.
- **Do** give every provider exactly one fixed family hue (Claude orange, Codex blue,
  MiniMax violet, Agent orchid) — never a severity hue — and shade models within
  their provider's family by in-scope rank.
- **Do** set `font-variant-numeric: tabular-nums` on every live or compared number so
  readouts never reflow.
- **Do** structure with 1px hairlines and flat graphite panels. Borders define surfaces.
- **Do** make values bright (`readout-bright`) and labels dim (`label`); build hierarchy
  with brightness and weight, not new hues.
- **Do** invert density by mode: pack the PFD (2/4/6/8px rhythm), give the Systems Pages
  room (8/10/12/16px), but keep one control vocabulary across both.
- **Do** keep motion functional and fast (0.15s on state, 0.3s on meter fills) and honor
  `prefers-reduced-motion`.

### Don't:
- **Don't** ship a generic SaaS template: no gradient hero, no big-number metric panels,
  no pill buttons, no identical icon-heading-text card grids.
- **Don't** reach for AI-hype / crypto finishes: no neon gradients, no glassmorphism, no
  glow, no colored shadows. A shadow only ever means "this floats."
- **Don't** go playful-consumer: no bubbly palette, no corner radius above 8px (pills
  excepted), no emoji, no mascots.
- **Don't** drift corporate-enterprise: no stock-photo blue, no gray-on-gray with acres
  of padding and three real numbers on the screen.
- **Don't** let a provider change color between panels, and don't let any category hue
  borrow a status color. Claude is orange everywhere or it's broken.
- **Don't** focus inputs in green (it collides with the meter) — focus is `signal-blue`.
- **Don't** use proportional figures for live numbers; jittering digits are an instrument
  defect.
- **Don't** cram editable controls — search, forms, settings — into the always-on PFD;
  they belong in the Systems Pages.
- **Don't** add another launcher window to the titlebar. The six tool windows are already
  a junk drawer; consolidate them, don't extend them.
