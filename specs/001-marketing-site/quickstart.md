# Quickstart — Maintainer Workflow

Refresh Quill's README and marketing screenshots without opening a window on the maintainer's desktop or reading personal Quill state.

## Prerequisites

- Docker with a running Linux container engine.
- Enough free space for the first Tauri/WebKitGTK build.
- Python 3 for local site preview.
- A modern browser for responsive and Lighthouse checks.

The capture image installs its own Rust, Node, WebKitGTK, Xvfb, Openbox, xcompmgr, xdotool, ImageMagick, and Python dependencies. The host does not need those tools.

## 1. Capture every canonical screenshot

```bash
./scripts/capture_screenshots_docker.sh
```

The command:

1. Builds `Dockerfile.screenshots` with the current frontend embedded through `tauri/custom-protocol`.
2. Starts a private 1280×1024 Xvfb desktop with Openbox and xcompmgr.
3. Runs `scripts/run_quill_demo.sh --clean` inside the container.
4. Seeds deterministic fictional usage, model, session, learning, memory, context, and provider-state data. A harmless `pi --version` stub makes the fictional Pi integration render as enabled.
5. Captures Usage, Models, Context, Sessions, Learning, Memories, Integrations, and Context/Brevity.
6. Checks every expected PNG before copying anything to the checkout.
7. Replaces `marketing-site/assets/screenshots/` only after the complete run passes.

The runtime container has no host display socket, personal home directory, Quill data directory, or published network port. It runs with Docker networking disabled. Only the repository build context enters the image; only validated PNGs leave it.

The first build downloads the Linux/Tauri toolchain. Docker BuildKit caches npm, Cargo registry, and Cargo target data for later runs.

## 2. Review the images

Open every PNG under `marketing-site/assets/screenshots/` and confirm:

- `hero.png`: Usage, 6H, model grouping, varied curves, and Skills rows with Claude/Codex/Pi counts.
- `models.png`: Models, 7D, current Claude/Codex/Pi model evidence and a ranked list.
- `analytics-context.png`: Context, 6H, preserved/retrieved/routing values.
- `sessions.png`: the `parser` query and a selected result detail.
- `learning.png`: active rules above discovered candidates.
- `memory.png`: `All Projects (4)` and four fictional memory files.
- `settings.png`: Claude Code, Codex, and Pi enabled; MiniMax remains below the crop.
- `brevity.png`: Context settings with Brevity ON.
- Every path, host, project, branch, and model identifier is fictional.

A blank capture fails automatically. This visual review catches a nonblank but incorrect UI state.

## 3. Preview the site

```bash
python3 -m http.server -d marketing-site 8000
```

Visit `http://localhost:8000` and check every anchor from `#hero` through `#install`. Resize to 320px wide and a large desktop width. Confirm screenshots remain whole, text stays readable, and no horizontal scrolling appears.

## 4. Run project checks

```bash
npm test
npm run typecheck
npm run lint
npm run knip
```

Run Lighthouse mobile and desktop checks when marketing HTML/CSS changes. Screenshot-only refreshes still need a visual page pass because source dimensions and aspect ratios can change.

## 5. Commit and deploy

Stage the documentation, marketing, screenshot, capture, spec, and LAT files that changed. Merging to `main` with changes under `marketing-site/**` triggers `.github/workflows/pages.yml`.

After deployment:

- Open every new anchor directly.
- Confirm the refreshed screenshot query versions load.
- Check the OpenGraph image.
- Confirm source and release links.

## Lower-level debugging

The host scripts remain available when diagnosing the capture workflow:

```bash
./scripts/run_quill_demo.sh --clean
./scripts/take_screenshots.sh
```

That path uses the host X11 session and can move focus or the pointer. Do not use it for routine publishing.

## Troubleshooting

| Symptom | Fix |
|---|---|
| Docker is unavailable | Start the installed Docker engine; the wrapper exits before changing screenshots. |
| First build takes several minutes | Expected: WebKitGTK packages and the release binary are cold. Later builds reuse caches. |
| Window opens but capture is blank | Keep `WEBKIT_DISABLE_COMPOSITING_MODE=1`, `WEBKIT_DISABLE_DMABUF_RENDERER=1`, Mesa software rendering, and xcompmgr in the container entrypoint. |
| Models view is empty | Confirm the seeder writes carry-forward `derived_model_id` values before the app folds hourly model rollups. |
| Learning has no active rules | Confirm the demo setting `legacy_rules_archived=1` is seeded before the current fixture rule files are written. |
| Sessions lacks detail | Check the fixed 960×680 query/result composition in `scripts/take_screenshots.sh`. |
| Images changed but Pages shows old versions | Bump every matching `?v=N` reference in `marketing-site/index.html`. |

## Independent test mapping

| Spec user story | Workflow coverage |
|---|---|
| US1 — visitor comprehension | Steps 2, 3, and post-deploy review |
| US2 — feature deep-dives | Steps 1–3 |
| US3 — maintainer dummy-data flow | Step 1 plus lower-level debugging |
| US4 — technical fit | Steps 3–5 |
