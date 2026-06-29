# Product

## Register

product

## Users

Developers who run AI coding agents — Claude Code, Codex, and MiniMax — and need
full visibility into what those agents are burning and doing, without leaving the
desktop or shipping anything to a cloud.

They are mid-flow in an agent-driven coding session. The default surface is a
compact, always-on-top widget glanceable in a screen corner; deeper windows
(analytics, full-text session search, learning, settings) open on demand. Their
job is to know exactly where tokens, sessions, and rate limits are going, search
across every past agent run, and hand their agents tools they use themselves —
all locally, on the user's own plan.

## Product Purpose

Quill is a local, cross-platform desktop console (Tauri + React 19) for AI coding
agents. It accounts for every token, session, and rule the agents touch: live
rate-limit pressure, historical analytics with breakdowns by host/project/session,
full-text search across all past sessions, behavioral-rule extraction, a memory
optimizer for agent instruction files, and MCP tools the agents call directly
(context preservation that keeps large transient data out of LLM transcripts).

Success is when the operator trusts the numbers at a glance and never has to guess
what their agents are doing or spending. It runs on the user's own plan — no API
key, no cloud, no tracking — and is open source (MIT) across macOS, Linux, and
Windows.

## Brand Personality

Precise, technical, instrument-grade. Quill is a calm, exact instrument for power
users — signal over noise. The voice is direct and unembellished: it states the
number, not the adjective; expert confidence, never hype. The interface should
read like a well-made measuring instrument — an oscilloscope, a flight console —
not a consumer app and not a marketing dashboard.

## Anti-references

This must explicitly NOT look like:

- **Generic SaaS template** — rounded cards everywhere, gradient hero blocks, pill
  buttons, big-number hero-metric panels, identical icon-heading-text card grids.
- **AI-hype / crypto** — neon gradients, glassmorphism, glow effects, breathless
  "revolutionary / supercharge your workflow" copy.
- **Playful consumer app** — bright bubbly palettes, heavy corner rounding, emoji,
  mascot energy, oversized friendly illustrations.
- **Corporate enterprise** — stiff stock-photo blue, marketing fluff, dense
  committee-designed blandness.

The shipped design already commits against these: dark `#121216` base, ~11px
system fonts, ≤6px radius, semantic status colors, density over decoration.

## Design Principles

1. **Instrument, not dashboard.** Every pixel earns its place by conveying state or
   data. Decoration that doesn't measure something gets removed.
2. **Signal over noise.** Density is a feature. Show the number the operator needs
   at a glance; suppress everything that competes with it.
3. **Local and honest.** The tool reports ground truth from the user's own machine
   and plan — no inflation, no cloud, no tracking. Trust is the product.
4. **One vocabulary, eight windows.** Same control shapes, same semantic colors
   (green = healthy, yellow = warning, red = error, blue = interactive), same
   affordances everywhere. Familiarity is the feature, not surprise.
5. **Stay out of the way.** This is an always-on companion to a coding session, not
   a destination — glanceable, fast, and never demanding attention it didn't earn.

## Accessibility & Inclusion

Dev-tool baseline, scoped to a power-user audience. Cover the fundamentals well:
keyboard navigability (the app already drives zoom, divider resizing, and list
navigation from the keyboard), visible focus rings, and sufficient text contrast
against the dark surface. Respect `prefers-reduced-motion` wherever motion exists.
Not targeting formal WCAG AA conformance testing — invest in keyboard and contrast
basics rather than over-engineering beyond the audience.
