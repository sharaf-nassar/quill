# Quickstart — Maintainer Workflow

How to refresh the marketing site (especially screenshots) end-to-end, without touching your personal Quill state.

## Prerequisites

- A working Quill installation OR a checkout you can build (`cargo build --release` from `src-tauri/`).
- Python 3 (already required for the existing seeder).
- POSIX shell (Linux / macOS) or PowerShell (Windows).
- For Linux screenshot capture only: `xdotool` and ImageMagick (`import`) — already required by `scripts/take_screenshots.sh`.
- A modern browser to preview the site locally (Chromium-based for Lighthouse verification).

## 1. Spin up a sandboxed Quill instance

```bash
# POSIX (Linux/macOS)
scripts/run_quill_demo.sh                      # uses /tmp/quill-demo-$USER
scripts/run_quill_demo.sh --clean              # nuke and reseed first
scripts/run_quill_demo.sh --bin /custom/quill  # use a specific binary
```

```powershell
# Windows
scripts/run_quill_demo.ps1
scripts/run_quill_demo.ps1 -Clean
```

The launcher prints the sandbox path on start, e.g.:

```text
[demo] sandbox at /tmp/quill-demo-alex
[demo] launching quill ...
```

A Quill window opens, populated with the deterministic dummy data set (Alex's projects, plausible token volumes, sample learned rules). Your real `~/.local/share/com.quilltoolkit.app/` is NOT touched — confirm by running `ls -lh ~/.local/share/com.quilltoolkit.app/` before and after; the timestamps should be unchanged.

## 2. Capture screenshots

With the demo window on screen, run the screenshot driver. On Linux:

```bash
scripts/take_screenshots.sh
```

This produces PNGs under `screenshots/` at the repo root. After the marketing-site landing, the script writes its outputs directly into `marketing-site/assets/screenshots/` (one of the script-extension tasks).

For views the existing driver doesn't yet capture (Settings, Context tab, Release Notes), capture manually with your platform tool:
- Linux: `import -window <wid> file.png`
- macOS: `screencapture -R x,y,w,h file.png` or `Cmd+Shift+4`
- Windows: Snipping Tool, or `Get-Clipboard | Save-Image`

Save into `marketing-site/assets/screenshots/` using the [naming convention](./data-model.md#3-screenshot-asset-naming).

**Privacy gate before continuing**: open every PNG and visually scan for any non-fictional identifier. If you see anything that isn't `/home/alex/projects/...`, `macbook-pro`, `dev-server`, `workstation`, etc., recapture before committing.

## 3. Preview the site locally

```bash
# Any of these works:
python3 -m http.server -d marketing-site 8000
# or
npx http-server marketing-site -p 8000 --no-cache
# or just:
xdg-open marketing-site/index.html             # Linux
open marketing-site/index.html                 # macOS
```

Visit `http://localhost:8000` and walk every anchored section (`#hero` through `#install`). Resize the browser to 320 px wide and to a 4K-ish width to confirm responsive correctness (FR-023).

## 4. Run Lighthouse before merging

In Chrome / Edge DevTools:
- Open DevTools → Lighthouse panel
- Categories: Performance + Accessibility + Best Practices + SEO
- Form factor: Mobile + Desktop, run both
- Confirm Performance ≥ 90 on both (SC-004)
- Largest Contentful Paint < 2.0 s on the desktop run (SC-005)
- Cumulative Layout Shift < 0.1 (SC-006)

If Performance dips, the most likely cause is an oversized PNG. Re-export at @2x and inspect file size; aim to keep each screenshot under ~150 KB.

## 5. Tear down the sandbox

```bash
# POSIX
rm -rf /tmp/quill-demo-$USER
```

```powershell
# Windows
Remove-Item -Recurse -Force $env:TEMP\quill-demo-$env:USERNAME
```

The launcher prints the exact teardown command on exit; copy-paste it.

## 6. Commit & push

Stage only the marketing-site changes (the env-var override Rust changes are a separate commit landing once across the repo, not per screenshot refresh):

```bash
git add marketing-site/
git commit -m "marketing-site: refresh screenshots after <change>"
git push origin <branch>
```

Open a PR. On merge to `main`, the GitHub Actions Pages workflow runs and the live URL updates within ~1 minute.

## 7. Verify the live deploy

After the Actions run finishes:
- Click the green check on the merge commit → "View deployment" → opens the deployed URL.
- Confirm the new screenshots are visible.
- Confirm the OG preview by pasting the URL into a chat client (Slack, Discord, X) — the social card should render `og-image.png`.

## Troubleshooting

| Symptom                                                            | Likely cause / fix                                                                 |
|--------------------------------------------------------------------|-------------------------------------------------------------------------------------|
| Demo Quill opens but shows your real data                          | `QUILL_DEMO_MODE` not set. Re-run via the launcher; do NOT export the env vars manually. |
| Demo Quill shows empty analytics                                   | Seeder didn't run. Try `scripts/run_quill_demo.sh --clean` to force a reseed.       |
| Lighthouse Performance drops to 70-something                       | Probably a PNG over ~300 KB. Re-export tighter; consider PNG-8 for low-color shots. |
| Pages workflow stays "queued" forever                              | Concurrency lock from a previous run. Cancel the queued job in the Actions UI.     |
| Screenshot driver complains "no window titled Quill"               | Demo Quill not on screen yet, or `xdotool` not installed (Linux only).             |
| OG preview shows a generic Pages icon                              | `og-image.png` missing or `<meta>` tag wrong. Validate with the LinkedIn / Twitter / OG previewer. |

## Independent test mapping

| Spec user story                | Quickstart step covering it |
|--------------------------------|------------------------------|
| US1 — visitor comprehension     | Step 3 + Step 7              |
| US2 — feature deep-dives        | Step 2 + Step 3 + Step 7     |
| US3 — maintainer dummy-data flow| Steps 1, 2, 5                |
| US4 — technical fit             | Step 7 (Install section live)|
