# Site Anchor Contract

This contract declares the seven URL fragment identifiers the marketing site exposes and commits to keeping stable. External documents (READMEs, blog posts, social shares) may deep-link to these anchors; the site MUST NOT rename or remove them without a deliberate breaking-change pass.

## Anchored sections (canonical order)

| Anchor        | Section heading (visible)                    | Content                                                                  |
|---------------|----------------------------------------------|--------------------------------------------------------------------------|
| `#hero`       | (no visible heading — top of page)           | Hero block: name, value-prop, primary CTA, primary screenshot           |
| `#live`       | "Know the cap before it cuts the run."       | Description + `live.png` showing pace marker, summary rail              |
| `#analytics`  | "Session metrics that answer operational questions." | Description + `analytics-now.png` and `analytics-charts.png`            |
| `#context`    | "Large content becomes a reference instead of a paste." | Description + context-savings proof                                      |
| `#search`     | "Past runs become an index, not a memory test." | Description + `sessions.png`                                             |
| `#learning`   | "Repeated behavior becomes a rule you can inspect." | Description + `learning.png`                                             |
| `#install`    | "Put the instrument panel beside the agents you already use." | Provider/platform compatibility, links to repo and releases              |

## Behavioral guarantees

1. **Permanence**: each anchor ID MUST exist on the deployed page. Renaming or removing an ID is a breaking change.
2. **Direct hit**: navigating directly to `…/#analytics` MUST scroll to (or paint with) that section in view, accounting for any sticky header offset.
3. **Order stability**: the canonical order above is the visual reading order. Reordering is permitted but MUST keep all seven anchors present.
4. **No client-side router**: the site does NOT use the History API to rewrite paths. The hash is the only routing signal.
5. **Smooth scroll** (when JavaScript is enabled): in-page anchor clicks SHOULD smooth-scroll. With JS disabled, the browser default jump is acceptable.

## Out-of-scope future anchors

Not part of the v1 contract; reserved for future expansion (a new clarification or follow-up spec):

- `#install/macos`, `#install/linux`, `#install/windows` — per-platform deep links
- `#changelog`, `#docs` — separate documentation surfaces
- `#privacy`, `#license` — secondary policy pages

## Test surface

A single integration check (manual or scripted) MUST verify that `<section id="hero">` through `<section id="install">` all exist in the published `index.html`. A trivial `grep` against the deployed HTML covers it.
