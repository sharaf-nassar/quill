# marketing-site

Source for the [Quill](https://github.com/sharaf-nassar/quill) marketing site. Static HTML/CSS/JS, no build step. Deployed to GitHub Pages by `.github/workflows/pages.yml` on every merge to `main` that touches files under this directory.

## Layout

```text
marketing-site/
├── index.html              Single page; nine anchored sections (analytics + agent tools)
├── styles.css              Signal Theater theme; no framework; self-hosted woff2 fonts
├── motion.js               Progressive native scroll-reveal (motion-rise) only
├── README.md               This file
└── assets/
    ├── logo.png            Real Quill app icon (tiled) — used as favicon
    ├── logo-mark.png       Borderless feather mark (app-icon frame stripped) — header brand
    ├── og-image.png        1200×630 social-share preview — logo mark beside the widget's
    │                       Usage frame with its Charts view behind; composed from the
    │                       shots below, carries no baked-in copy, recompose on recapture
    ├── fonts/              Self-hosted woff2 — Space Grotesk (display) + Geist (body), OFL
    └── screenshots/        @2x captures from the dummy-data Quill instance
        ├── hero.png            The widget on its Usage view — the whole 360px window
        ├── live.png            Same frame; the #live section clips it to LIMITS
        ├── analytics-charts.png  The widget on its Charts view (the #analytics section)
        ├── analytics-context.png The widget on its Context view (the #context section)
        ├── sessions.png         Manage → Sessions, a query typed and a result open
        ├── learning.png         Manage → Learning, active rules over candidates
        ├── memory.png          Memories panel — "All Projects (4)" (the #memory section)
        └── brevity.png         Brevity profile toggle (the #brevity section)
```

The main window is the 360px widget, so the first four PNGs are all the same
window on different views rather than four separate surfaces. The last four
come from the Manage workspace: `sessions.png` and `learning.png` are whole
sections captured by `take_screenshots.sh`, while `memory.png` and
`brevity.png` are sub-panels inside a section and stay manual.

## Anchored sections

The page exposes stable URL fragments — part of the public deep-link surface. See [contracts/site-anchors.md](../specs/001-marketing-site/contracts/site-anchors.md). The 2026-06-19 redesign reordered the narrative (analytics first, then the agent tools built on it) and added two anchors:

- `#hero` — value proposition + primary install CTA
- `#analytics` — complete analytical insight (lead flagship)
- `#context` — context offloading / working memory (the agent calls Quill's MCP tools)
- `#search` — session search
- `#live` — live limits
- `#learning` — learning system
- `#memory` — memory tools (added 2026-06-19)
- `#brevity` — brevity / prose compression (added 2026-06-19)
- `#install` — providers, platforms, privacy, repo links

The original seven (`#hero`, `#live`, `#analytics`, `#context`, `#search`, `#learning`, `#install`) are a stable contract — renaming or removing any of them is a breaking change. `#memory` and `#brevity` are additive.

## Visual direction

[Signal Theater](../specs/001-marketing-site/spec.md#clarifications) — revised 2026-05-12. The page reads like a premium desktop instrument panel for agent work: Quill's quiet dark app surface, the real quill logo mark, cyan/purple logo accents, clipped geometry, dense screenshot proof, native scroll-reveal motion, and no generic SaaS cards.

## Screenshot display & section layout

The screenshots are captured "lean" — each PNG is tightly cropped to its own
content (no excess window chrome, no scrollbars, no dead space) and they have
varied aspect ratios. They are displayed **whole at their natural aspect ratio**
— never cropped, never letterboxed.

- Every screenshot lives in a `.shot` frame: a thin `rgba(192,202,245,0.10)`
  hairline border, soft shadow, ≤6px radius, and a dark `#08090c` matte behind
  the PNG's transparent edges. The `<img>` is `width: 100%; height: auto;
  display: block` — no `object-fit: cover`, no fixed height. Each `<img>` also
  carries explicit `width`/`height` attributes matching its 2× source so the
  browser reserves correct space (no layout shift) and the aspect ratio is right.
- The PNGs are stored at 2× for retina; their display sizes are half the pixel
  dimensions (e.g. `hero.png` 720×1532 → 360×766).
- **Slim, never upscaled.** Each `.spotlight` sets a `--shot-w` custom property
  at or below the shot's native retina display width (its `width` attribute), and
  the media grid track is `minmax(0, var(--shot-w, 480px))`. So the product
  window renders at — or below — its captured size, never stretched wider or
  taller to fill the column. The four widget shots use 360px (the widget's own
  width); the landscape Manage shots use 640px and the two sub-panel shots
  600px — each still at or under its own captured width. The copy column
  (`1fr`) takes the remaining width.
- **One exception to "shown whole":** `#live` reuses the widget frame through a
  `.shot-band` frame with `aspect-ratio: 360 / 185`, clipping it to the LIMITS
  band at that band's own hairline. The widget is a single window, so LIMITS has
  no capture of its own, and clipping beats publishing the hero shot twice. The
  image itself is still rendered at native width and never scaled.
- Feature sections use a single, consistent **alternating two-column
  `.spotlight` rhythm**: copy on one side, the slim screenshot on the other,
  sides flipping down the page (`.spotlight-reverse` swaps order). `#hero` stays
  in the right column beside the copy as `hero.png` (the widget on its Usage
  view) on a 360px stage — shown whole, with no height clip or bottom fade,
  because the widget frame ends at its own footer row; the hero collapses to a
  single centered column under 980px.
- On `<980px` each spotlight collapses to a single column (copy then image) with
  no horizontal scroll; the `--shot-w` cap still prevents any upscaling.

## Preview locally

```sh
python3 -m http.server -d marketing-site 18080
# then visit http://localhost:18080/
```

The page is pure HTML/CSS, so any static file server works. Live-reload is not needed.

## Refreshing screenshots

Screenshots come from a sandboxed Quill instance — never the maintainer's personal Quill. The full workflow is in [`specs/001-marketing-site/quickstart.md`](../specs/001-marketing-site/quickstart.md). Short version:

```sh
# 1. Spin up a sandboxed Quill against dummy data
./scripts/run_quill_demo.sh --clean

# 2. Capture the canonical widget and Manage PNGs into
#    marketing-site/assets/screenshots/ (memory.png and brevity.png are manual)
./scripts/take_screenshots.sh

# 3. Privacy review — open every PNG, confirm only fictional identifiers
#    (look for /home/alex/projects/..., dev-server, macbook-pro, etc.)

# 4. Tear down the sandbox
rm -rf /tmp/quill-demo-$USER
```

The launcher uses env-var path overrides (`QUILL_DEMO_MODE=1`, `QUILL_DATA_DIR`, `QUILL_RULES_DIR`, `QUILL_CLAUDE_PROJECTS_DIR`, `QUILL_CODEX_SESSIONS_DIR`) so the maintainer's `~/.local/share/com.quilltoolkit.app/`, `~/.claude/`, and `~/.codex/` are never touched. See [`specs/001-marketing-site/contracts/env-vars.md`](../specs/001-marketing-site/contracts/env-vars.md).

## Editing rules

- Add or remove a feature section: edit `<section>` blocks in `index.html`. Anchor IDs above are stable.
- Add or remove a screenshot: place the PNG under `assets/screenshots/`, reference it from the appropriate `<figure>`. Filenames map 1:1 to anchored sections.
- Change visual CSS: bump the `styles.css?v=...` query in `index.html` so local previews and Pages visitors do not keep stale cached styles.
- Replace a screenshot in place (same filename, new content): bump its `?v=N` query on every `<img src>`/preload reference in `index.html`. Browsers cache images by URL and may serve a stale cached copy otherwise — re-capturing without bumping leaves visitors looking at the old shot.
- Visual direction stays Signal Theater — see [spec.md § Clarifications](../specs/001-marketing-site/spec.md#clarifications). Avoid generic SaaS-landing-page conventions.
- No tracking scripts, no third-party analytics, no *remote* fonts (FR-028, FR-007). Display/body fonts are self-hosted woff2 under `assets/fonts/` (Space Grotesk, Geist — OFL), served same-origin and preloaded.
- Page MUST stay readable with JavaScript disabled (FR-024). Native scroll reveal is progressive enhancement only; core content, anchors, links, and screenshots must work when scripts fail or motion is reduced.

## Deploy

Merging to `main` with changes under `marketing-site/**` triggers the [`Pages` workflow](../.github/workflows/pages.yml). The deployed URL surfaces in the Actions UI under the `github-pages` environment. Manual redeploys (e.g., after rotating screenshots) are available via the Actions UI's `workflow_dispatch` button.

Full contract: [`specs/001-marketing-site/contracts/pages-workflow.md`](../specs/001-marketing-site/contracts/pages-workflow.md).
