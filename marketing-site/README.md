# marketing-site

Source for the [Quill](https://github.com/sharaf-nassar/quill) marketing site. Static HTML/CSS, no build step. Deployed to GitHub Pages by `.github/workflows/pages.yml` on every merge to `main` that touches files under this directory.

## Layout

```text
marketing-site/
├── index.html              Single page; seven anchored sections
├── styles.css              Terminal Console theme; no framework, no web fonts
├── README.md               This file
└── assets/
    ├── favicon.svg         32×32 favicon (light/dark scheme aware)
    ├── og-image.png        1200×630 social-share preview
    └── screenshots/        @2x captures from the dummy-data Quill instance
        ├── hero.png
        ├── live.png
        ├── analytics-now.png
        ├── analytics-charts.png
        ├── analytics-context.png
        ├── sessions.png
        └── learning.png
```

## Anchored sections

The page exposes seven stable URL fragments — these are part of the public deep-link surface. See [contracts/site-anchors.md](../specs/001-marketing-site/contracts/site-anchors.md):

- `#hero` — conversion shot
- `#live` — live usage feature
- `#analytics` — analytics dashboard with the FR-012 LLM-help narrative
- `#context` — context savings
- `#search` — session search
- `#learning` — learning system
- `#install` — providers, platforms, repo links

Renaming or removing any of these is a breaking change.

## Visual direction

[Terminal Console](../specs/001-marketing-site/spec.md#clarifications) — chosen 2026-05-08. Mirrors the desktop app's dark theme (`#121216` background, `#d4d4d4` text, semantic green/yellow/red status colors, blue accent). Monospace stack, no Inter, square-defaulting `≤6px` border radius.

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

# 2. Capture all 7 canonical PNGs into marketing-site/assets/screenshots/
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
- Visual direction stays Terminal Console — see [spec.md § Clarifications](../specs/001-marketing-site/spec.md#clarifications). Avoid generic SaaS-landing-page conventions.
- No tracking scripts, no third-party analytics, no remote fonts (FR-028, FR-007).
- Page MUST stay readable with JavaScript disabled (FR-024). The site currently has zero `<script>` tags — keep it that way unless you have a specific reason.

## Deploy

Merging to `main` with changes under `marketing-site/**` triggers the [`Pages` workflow](../.github/workflows/pages.yml). The deployed URL surfaces in the Actions UI under the `github-pages` environment. Manual redeploys (e.g., after rotating screenshots) are available via the Actions UI's `workflow_dispatch` button.

Full contract: [`specs/001-marketing-site/contracts/pages-workflow.md`](../specs/001-marketing-site/contracts/pages-workflow.md).
