# marketing-site

Source for the [Quill](https://github.com/sharaf-nassar/quill) marketing site. Static HTML/CSS/JS, no build step. Deployed to GitHub Pages by `.github/workflows/pages.yml` on every merge to `main` that touches files under this directory.

## Layout

```text
marketing-site/
├── index.html              Single page; eleven anchored product sections
├── styles.css              Signal Theater theme; no framework; self-hosted woff2 fonts
├── motion.js               Progressive native scroll-reveal (motion-rise) only
├── README.md               This file
└── assets/
    ├── logo.png            Real Quill app icon (tiled) — used as favicon
    ├── logo-mark.png       Borderless feather mark (app-icon frame stripped) — header brand
    ├── fonts/              Self-hosted woff2 — Space Grotesk (display) + Geist (body), OFL
    └── screenshots/        @2x captures from the dummy-data Quill instance
        ├── hero.png             Widget → Usage for #hero, #analytics, and social previews
        ├── live.png             Same frame; #live clips it to the LIMITS band
        ├── models.png           Widget → Models for #models
        ├── analytics-context.png Widget → Context for #context
        ├── sessions.png         Tools → Sessions, with a query and detail open
        ├── learning.png         Tools → Learning → Rules
        ├── memory.png           Tools → Learning → Memories
        ├── settings.png         Tools → Settings → Integrations
        └── brevity.png          Tools → Settings → Context, showing Brevity
```

The main window is the 360px widget. `hero.png` serves both `#hero` and
`#analytics`; `live.png` copies that Usage frame for `#live`; `models.png` and
`analytics-context.png` show the other two widget views. Tools images use the
full 960×680 workspace except `settings.png`, cropped after Pi to omit the
currently deferred MiniMax row.

## Anchored sections

The page exposes stable URL fragments as a public deep-link surface. See [contracts/site-anchors.md](../specs/001-marketing-site/contracts/site-anchors.md). The current narrative starts with usage/model analytics, then the agent tools built on them:

- `#hero` — value proposition + primary install CTA
- `#analytics` — measured usage photographed in model grouping
- `#models` — current and session-ranked model evidence
- `#context` — context offloading / working memory (the agent calls Quill tools)
- `#search` — session search
- `#live` — live limits
- `#learning` — learning system
- `#memory` — memory tools (added 2026-06-19)
- `#brevity` — brevity / prose compression (added 2026-06-19)
- `#integrations` — Claude Code, Codex, Pi, and pooled usage sources
- `#install` — providers, platforms, privacy, repo links

The original seven (`#hero`, `#live`, `#analytics`, `#context`, `#search`, `#learning`, `#install`) are a stable contract — renaming or removing any of them is a breaking change. `#memory`, `#brevity`, `#models`, and `#integrations` are additive.

## Visual direction

[Signal Theater](../specs/001-marketing-site/spec.md#clarifications) — revised 2026-05-12. The page reads like a premium desktop instrument panel for agent work: Quill's quiet dark app surface, the real quill logo mark, cyan/purple logo accents, clipped geometry, dense screenshot proof, native scroll-reveal motion, and no generic SaaS cards.

## Screenshot display & section layout

The screenshot set uses two fixed surfaces: the 360×800 widget and the 960×680
Tools workspace. PNGs are stored at 2× and displayed whole at or below their
native logical size. Only `#live` clips the shared widget image to its LIMITS band.

- Every screenshot lives in a `.shot` frame: a thin `rgba(192,202,245,0.10)`
  hairline border, soft shadow, ≤6px radius, and a dark `#08090c` matte behind
  the PNG's transparent edges. The `<img>` is `width: 100%; height: auto;
  display: block` — no `object-fit: cover`, no fixed height. Each `<img>` also
  carries explicit `width`/`height` attributes matching its 2× source so the
  browser reserves correct space (no layout shift) and the aspect ratio is right.
- The PNGs are stored at 2× for retina: widget images are 720×1600, full Tools
  images are 1920×1360, and `settings.png` is 1920×1020.
- **Slim, never upscaled.** Each `.spotlight` sets a `--shot-w` custom property
  at or below the shot's native retina display width (its `width` attribute), and
  the media grid track is `minmax(0, var(--shot-w, 480px))`. So the product
  window renders at — or below — its captured size, never stretched wider or
  taller to fill the column. Widget shots use 360px; Tools shots use 640px.
  The copy column (`1fr`) takes the remaining width.
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

Screenshots come from a deterministic Quill instance inside a private Docker
X11 desktop. The container receives no host display socket, personal home,
Quill data directory, or published port.

```sh
./scripts/capture_screenshots_docker.sh
```

The command builds the current release binary with Tauri's custom protocol,
starts Xvfb, Openbox, and a compositor, seeds fictional Claude/Codex data, drives
all widget and Tools compositions, and replaces the canonical PNGs only after
every expected output passes. Docker and Cargo caches make later runs much
faster than the first.

The lower-level host launcher remains available for debugging, but it is no
longer the publishing workflow. See
[`specs/001-marketing-site/quickstart.md`](../specs/001-marketing-site/quickstart.md).

## Editing rules

- Add or remove a feature section: edit `<section>` blocks in `index.html`. Anchor IDs above are stable.
- Add or remove a screenshot: place the PNG under `assets/screenshots/` and reference it from every appropriate `<figure>`. One capture may serve multiple anchored sections.
- Change visual CSS: bump the `styles.css?v=...` query in `index.html` so local previews and Pages visitors do not keep stale cached styles.
- Replace a screenshot in place (same filename, new content): bump its `?v=N` query on every `<img src>`/preload reference in `index.html`. Browsers cache images by URL and may serve a stale cached copy otherwise — re-capturing without bumping leaves visitors looking at the old shot.
- Visual direction stays Signal Theater — see [spec.md § Clarifications](../specs/001-marketing-site/spec.md#clarifications). Avoid generic SaaS-landing-page conventions.
- No tracking scripts, no third-party analytics, no *remote* fonts (FR-028, FR-007). Display/body fonts are self-hosted woff2 under `assets/fonts/` (Space Grotesk, Geist — OFL), served same-origin and preloaded.
- Page MUST stay readable with JavaScript disabled (FR-024). Native scroll reveal is progressive enhancement only; core content, anchors, links, and screenshots must work when scripts fail or motion is reduced.

## Deploy

Merging to `main` with changes under `marketing-site/**` triggers the [`Pages` workflow](../.github/workflows/pages.yml). The deployed URL surfaces in the Actions UI under the `github-pages` environment. Manual redeploys (e.g., after rotating screenshots) are available via the Actions UI's `workflow_dispatch` button.

Full contract: [`specs/001-marketing-site/contracts/pages-workflow.md`](../specs/001-marketing-site/contracts/pages-workflow.md).
