# Quickstart — Maintainer Workflow

Refresh Quill's README and marketing screenshots from the real frontend without opening a window on the maintainer's desktop or reading personal state.

## Prerequisites

- Docker with a running Linux container engine.
- Enough disk space for the Node + Chromium capture image.
- Python 3 and a modern browser for local site review.

The host does not need Rust, WebKitGTK, Xvfb, xdotool, ImageMagick, or a running Quill instance.

## 1. Capture every canonical screenshot

```bash
./scripts/capture_screenshots_docker.sh
```

The command:

1. Builds `Dockerfile.screenshots` with the current React source and npm lockfile.
2. Starts a runtime container with networking disabled.
3. Runs Vite in Quill's documented Browser Mock Mode.
4. Opens the actual app entry point in headless Chromium at DPR 2.
5. Uses Chrome DevTools Protocol to operate the real view switcher, range controls, Tools rail, tabs, search input, result list, and scrolling containers.
6. Captures Usage, Models, Context, Sessions, Learning, Memories, Integrations, and Context/Brevity.
7. Validates all nine PNGs and their dimensions before copying anything to the checkout.
8. Replaces `marketing-site/assets/screenshots/` only after the complete run passes.

The `?screenshot=marketing` query is dev-only. It hides the visible `MOCK DATA` badge and selects the maintained marketing fixture profile in `src/mocks/ipcFixtures.ts`. It does not change production bundles or component behavior.

## 2. Review the images

Open every PNG under `marketing-site/assets/screenshots/` and confirm:

- `hero.png`: Usage, 6H, Model grouping, varied curves, and Claude/Pi/Codex session rows with agent models.
- `models.png`: Models, 7D, current Claude/Codex/Pi evidence and five ranked models.
- `analytics-context.png`: Context, 6H, preserved/retrieved/routing values.
- `sessions.png`: the `parser` query, Claude/Codex/Pi results, and selected context.
- `learning.png`: active rules above a discovered candidate.
- `memory.png`: `All Projects (4)` and four provider-aware files.
- `settings.png`: Claude Code, Codex, and Pi enabled; MiniMax absent.
- `brevity.png`: Context settings with Brevity ON.
- No `MOCK DATA` badge is present.

## 3. Preview the site

```bash
python3 -m http.server -d marketing-site 8000
```

Visit `http://localhost:8000`, check every anchor, then resize to 320px and a large desktop width. Screenshots must stay whole, text must remain readable, and no horizontal scrolling may appear.

## 4. Run project checks

```bash
npm run typecheck
npm run lint
npm test
npm run knip
npm run build
lat check
```

Run Lighthouse mobile and desktop checks when marketing HTML or CSS changes.

## 5. Commit and deploy

Stage the frontend fixtures, capture scripts, documentation, screenshots, specs, and LAT changes. Merging to `main` with changes under `marketing-site/**` triggers `.github/workflows/pages.yml`.

After deployment, open every anchor directly, confirm the bumped screenshot query versions load, and check the OpenGraph image.

## Lower-level backend debugging

The older sandboxed Tauri path remains available for backend and migration investigation:

```bash
./scripts/run_quill_demo.sh --clean
./scripts/take_screenshots.sh
```

It is not the publishing path. It seeds SQLite and retained JSONLs rather than using the frontend's maintained mock contract, and host capture can move focus or the pointer.

## Troubleshooting

| Symptom | Fix |
|---|---|
| Docker is unavailable | Start the installed Docker engine; the wrapper exits before changing screenshots. |
| Capture times out waiting for a selector | Inspect the named component selector in `scripts/capture_browser_screenshots.mjs`; UI navigation changed. |
| A section is empty | Update the corresponding handler in `src/mocks/ipcFixtures.ts`, not the Python database seeder. |
| MiniMax or the mock badge appears | Confirm the URL includes `screenshot=marketing` and the marketing profile filters both. |
| Images changed but Pages shows old versions | Bump every matching `?v=N` reference in `marketing-site/index.html`. |

## Independent test mapping

| Spec user story | Workflow coverage |
|---|---|
| US1 — visitor comprehension | Steps 2, 3, and post-deploy review |
| US2 — feature deep-dives | Steps 1–3 |
| US3 — maintainer isolated capture | Step 1 |
| US4 — technical fit | Steps 3–5 |
