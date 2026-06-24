# Site Anchor Contract

This contract declares the URL fragment identifiers the marketing site exposes and commits to keeping stable. The seven original anchors (`#hero`, `#live`, `#analytics`, `#context`, `#search`, `#learning`, `#install`) are the hard contract; `#memory` and `#brevity` were added 2026-06-19 and are additive. External documents (READMEs, blog posts, social shares) may deep-link to these anchors; the site MUST NOT rename or remove the original seven without a deliberate breaking-change pass.

## Anchored sections (canonical order)

| Anchor        | Section heading (visible)                              | Content                                                             |
|---------------|--------------------------------------------------------|---------------------------------------------------------------------|
| `#hero`       | (no visible heading — top of page)                     | Product-led hero: prelude, headline "Stop running your coding agents blind.", deck "You get the insight. Your agents get the tools.", lede, CTAs, trust line, large `hero.png` window |
| `#analytics`  | "Every token accounted for — not estimated."           | Description + `analytics-now.png` and `analytics-charts.png`         |
| `#context`    | "Keep bloated context out of the model's window."      | Agent-facing MCP working-memory tools + `analytics-context.png`     |
| `#search`     | "Every past run, indexed — Claude and Codex."          | Description + `sessions.png`                                         |
| `#live`       | "Know the cap before it cuts the run."                 | Description + `live.png`                                             |
| `#learning`   | "Your agent learns your rules — only after Quill proves they help." | Description + `learning.png`                            |
| `#memory`     | "Keep your agent's memory clean and current."          | Memory tools (text-only; no screenshot yet) — added 2026-06-19      |
| `#brevity`    | "Get tighter answers from your agents."                | Prose compression / brevity profile (text-only) — added 2026-06-19  |
| `#install`    | "Put it beside the agents you already use."            | Provider/platform/privacy, links to repo and releases               |

## Behavioral guarantees

1. **Permanence**: each anchor ID MUST exist on the deployed page. Renaming or removing an ID is a breaking change.
2. **Direct hit**: navigating directly to `…/#analytics` MUST scroll to (or paint with) that section in view, accounting for any sticky header offset.
3. **Order stability**: the canonical order above is the visual reading order. Reordering is permitted but MUST keep all seven original anchors (`#hero`, `#live`, `#analytics`, `#context`, `#search`, `#learning`, `#install`) present; `#memory` and `#brevity` are additive and may be reordered or removed without a breaking-change pass.
4. **No client-side router**: the site does NOT use the History API to rewrite paths. The hash is the only routing signal.
5. **Smooth scroll** (when JavaScript is enabled): in-page anchor clicks SHOULD smooth-scroll. With JS disabled, the browser default jump is acceptable.

## Out-of-scope future anchors

Not part of the v1 contract; reserved for future expansion (a new clarification or follow-up spec):

- `#install/macos`, `#install/linux`, `#install/windows` — per-platform deep links
- `#changelog`, `#docs` — separate documentation surfaces
- `#privacy`, `#license` — secondary policy pages

## Test surface

A single integration check (manual or scripted) MUST verify that `<section id="hero">` through `<section id="install">` all exist in the published `index.html`. A trivial `grep` against the deployed HTML covers it.
