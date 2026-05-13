# Phase 0 Research — Quill Marketing Site

This document records the technical decisions made during planning, why each was chosen, and what alternatives were considered. Each entry is independently revisitable; nothing here is a final architectural commitment beyond what the [spec](./spec.md) and [plan](./plan.md) lock in.

## R1. Static plain HTML/CSS vs. static-site generator

**Decision**: Hand-written `index.html` + `styles.css`. No build step, no SSG, no framework, no JavaScript for the shipped page.

**Rationale**:
- Q2 locked in a single page with seven anchored sections — total content is small enough to live in one HTML file without composability gymnastics.
- The page weight budget (FR-026) and `marketing-site/` layout (clarification answer for Q4) are easier to keep honest with no toolchain in the loop.
- Pairs cleanly with the chosen "GitHub Actions copies the directory" deploy contract — the workflow becomes a thin layer over `actions/upload-pages-artifact`.
- Removes a maintainer prerequisite: no Node toolchain needed to preview locally; `python3 -m http.server` or even `xdg-open` against the file works.

**Alternatives considered**:
- **Astro**: Reasonable for a marketing site, but adds a `node_modules` plus a build step for one page. Rejected as overkill for v1; reachable as a future migration if section composition gets unwieldy.
- **Eleventy (11ty)**: Smaller than Astro, but still pulls Node toolchain. Same rejection logic.
- **Hugo**: One Go binary, but introduces a Go template language for one page. Rejected.
- **MkDocs / Docusaurus**: These target documentation sites, not marketing landing pages. Wrong fit for this feature.

## R2. Typography choice (FR-007 forbids Inter)

**Decision**: Dependency-free local font stacks: a system serif stack for display headlines, a readable local sans stack for prose, and a system mono stack for labels and metric chrome:

```css
--font-display: "Cabinet Grotesk", "Arial Narrow", "Aptos Display", "Segoe UI", sans-serif;
--font-body: "Geist", "Aptos", "Segoe UI", system-ui, sans-serif;
--font-mono: ui-monospace, "SF Mono", "JetBrains Mono", "Fira Code", Menlo, Consolas, monospace;
```

Headlines use the Cabinet Grotesk-first stack for a dense instrument-panel feel. Mono stays reserved for navigation, frame chrome, metric labels, and technical tags.

**Rationale**:
- Honors FR-005 (Signal Theater aesthetic) and FR-007 (no Inter).
- Zero web-font cost — keeps page weight low (FR-026) and works fully offline.
- Each fallback is bundled with at least one mainstream OS, so most visitors see a polished local font without loading third-party assets.

**Alternatives considered**:
- **Self-hosted JetBrains Mono via woff2**: Adds 60–80 KB per weight. Rejected for v1 to stay inside the perf budget; can be re-introduced if the system stack proves visually inconsistent across platforms.
- **Inter / generic sans by default**: Explicitly forbidden by FR-007.
- **Remote display font for headlines + system body**: More moving parts, more fonts to QA, more bytes, and violates the no-third-party-load baseline. Rejected.

## R3. Rust env-var override pattern

**Decision**: Introduce `src-tauri/src/data_paths.rs` exposing two pure functions, `resolve_data_dir(app: &AppHandle) -> PathBuf` and `resolve_rules_dir() -> PathBuf`. Each function:

1. Checks `std::env::var("QUILL_DEMO_MODE")`. If it is not exactly `1`, ignores any override env var and returns the default (Tauri's `app_data_dir()` for data, the existing hard-coded learned-rules path for rules). This makes a stray env var in a maintainer's shell harmless.
2. If `QUILL_DEMO_MODE=1`, also reads the relevant override (`QUILL_DATA_DIR` or `QUILL_RULES_DIR`). If unset, falls back to the default and logs a warning. If set, returns the override path. Creates the path if it does not exist (the launcher does this too, so it is defensive).
3. Logs at startup (eprintln + tracing) when demo mode is active, including the resolved paths, so the maintainer can never confuse a demo run with a real one.

All current call-sites in `src-tauri/src/lib.rs` and adjacent modules that compute the data dir or learned-rules dir are routed through these two functions.

**Rationale**:
- Smallest possible surface — two functions, one module file, plus call-site swaps.
- Opt-in via `QUILL_DEMO_MODE=1` makes accidental redirection impossible: an env var set in `.zshrc` or by a parent process won't take effect without the explicit demo flag.
- Cross-platform: `app_data_dir()` resolves to `~/.local/share/com.quilltoolkit.app/` (Linux), `~/Library/Application Support/com.quilltoolkit.app/` (macOS), `%APPDATA%\com.quilltoolkit.app\` (Windows); the override replaces all three uniformly.
- Test surface is small: one file, three behavioral cases.

**Alternatives considered**:
- **Separate Tauri identifier (`com.quilltoolkit.app.demo`) for a demo build**: OS-isolated trivially, but doubles release config (`tauri.conf.json`, code signing, icons) and forces maintainers to build a second binary. Rejected for the cost.
- **CLI flag (`--data-dir`) only**: Doesn't help the many internal call-sites that already call `app_data_dir()`. Would also leave the rules-dir override un-handled. Rejected.
- **Read env var unconditionally (no `QUILL_DEMO_MODE` gate)**: Risky — a stray env var could redirect a user's real Quill silently. Rejected on safety grounds.

## R4. Cross-platform launcher shape

**Decision**: Two parallel scripts:
- `scripts/run_quill_demo.sh` — POSIX shell, used on Linux and macOS.
- `scripts/run_quill_demo.ps1` — PowerShell, used on Windows.

Both:
1. Compute a sandbox directory under the platform's temp dir (`/tmp/quill-demo-$USER` on POSIX, `$env:TEMP\quill-demo-$env:USERNAME` on Windows). The path is stable per user so re-running the launcher reuses the same dataset; an optional `--clean` / `-Clean` flag wipes it first.
2. Set `QUILL_DEMO_MODE=1`, `QUILL_DATA_DIR=$SANDBOX/data`, `QUILL_RULES_DIR=$SANDBOX/rules`.
3. Invoke the seeder: `python3 scripts/populate_dummy_data.py --data-dir "$QUILL_DATA_DIR" --rules-dir "$QUILL_RULES_DIR"`.
4. Exec the installed Quill binary (auto-discovered: tries `quill` on `$PATH`, then `target/release/quill`, then `target/debug/quill`; configurable via `--bin` flag).
5. Print the sandbox path and a teardown command (`rm -rf $SANDBOX`) on exit so the maintainer can clean up explicitly.

**Rationale**:
- Two scripts is the cheapest portable surface. Bash works on Linux and macOS; PowerShell ships with every Windows install.
- Stable sandbox path lets repeat captures share a dataset, which is desirable when iterating on screenshots.
- `--clean` / `-Clean` opt-in gives a reset path without making destruction the default.
- Auto-discovering the binary keeps the script useful both for a maintainer with Quill installed system-wide and for a developer running from a checkout.

**Alternatives considered**:
- **Single Python launcher**: Adds Python to the Windows demo-capture prereq list (Python isn't always installed on Windows dev machines). Rejected.
- **A Rust binary (e.g., `cargo xtask quill-demo`)**: Heavier than two scripts. Could revisit if the launcher logic grows.
- **Make-target / npm-script wrapper**: Adds a build-tool dependency for what should be a standalone capture path. Rejected.

## R5. Screenshot scope (which views to capture)

**Decision**: The dummy-data instance produces seven canonical PNGs (all @2x for HiDPI rendering per FR-021):

| Filename                       | View                                            | Section anchor   |
|--------------------------------|-------------------------------------------------|------------------|
| `hero.png`                     | Main window with Live + Analytics dual pane     | `#hero`          |
| `live.png`                     | Full Live pane: summary rail + grouped quotas   | `#live`          |
| `analytics-now.png`            | Analytics → Now tab with 6 insight cards        | `#analytics`     |
| `analytics-charts.png`         | Analytics → Charts tab composite                | `#analytics`     |
| `analytics-context.png`        | Analytics → Context tab strip + breakdown       | `#context`       |
| `sessions.png`                 | Session Search window with results              | `#search`        |
| `learning.png`                 | Learning window — Rules tab with several rules  | `#learning`      |

Optional follow-up captures (not v1): Settings window, Restart window, Release Notes window, Plugin Manager.

**Rationale**:
- Maps 1:1 to the anchored sections from Q2's clarification, so no section ships without a screenshot (FR-022).
- Includes Charts and Context tabs explicitly because the spec's Analytics narrative (FR-012) calls them out as showcasing how analytics help with LLM work.
- @2x captures honor FR-021 / SC-007.

**Alternatives considered**:
- **One composite shot for the hero, no per-feature shots**: Defeats the deep-dive purpose (US2). Rejected.
- **Capture every secondary window**: Makes the asset set unwieldy and the page heavier than necessary. Deferred to a v2 if needed.

## R6. GitHub Pages workflow shape

**Decision**: A single `.github/workflows/pages.yml` with two jobs (`build`, `deploy`), using the official Pages actions:

```yaml
name: Pages

on:
  push:
    branches: [main]
    paths:
      - "marketing-site/**"
      - ".github/workflows/pages.yml"
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: "pages"
  cancel-in-progress: false

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/configure-pages@v5
      - uses: actions/upload-pages-artifact@v3
        with:
          path: marketing-site/

  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - id: deployment
        uses: actions/deploy-pages@v4
```

**Rationale**:
- Path filter prevents app-only changes from triggering noisy deploys.
- `workflow_dispatch` lets a maintainer redeploy manually after rotating screenshots without pushing a commit.
- `concurrency: pages, cancel-in-progress: false` matches GitHub's recommended Pages pattern (don't cancel an in-flight deploy).
- Permissions follow the GitHub Pages least-privilege template.
- Two-job split is the canonical Pages flow; keeps the deploy step isolated and visible in the Actions UI.

**Alternatives considered**:
- **`peaceiris/actions-gh-pages`**: Older third-party action that pushes to a `gh-pages` branch. Functional but unnecessary now that GitHub provides first-party Pages actions. Rejected.
- **One job that does both upload and deploy**: Loses the `environment: github-pages` URL surfacing in the Actions UI. Rejected for observability cost.
- **Always-deploy on every commit (no path filter)**: Wastes CI minutes when only Quill app code changed. Rejected.

## R7. OG / social-share preview

**Decision**: Hand-built 1200×630 PNG (`marketing-site/assets/og-image.png`) composed of the hero screenshot and Signal Theater typography, with the headline "Quill gives agent work telemetry." Referenced via `<meta property="og:image">`, `<meta name="twitter:card" content="summary_large_image">`, and `<meta name="twitter:image">`.

**Rationale**:
- Meaningful preview > brand-only logo (per spec edge case "Social-share preview").
- Static PNG is the simplest, most universally-supported format for OG previews.
- 1200×630 is the recommended OG image dimension.

**Alternatives considered**:
- **Dynamically generated OG image** (e.g., an Edge Function or `@vercel/og`-style approach): Far too complex for v1; unavailable on plain GitHub Pages without external services. Rejected.
- **Logo-only preview**: Less informative; user explicitly wants to highlight features. Rejected.

## R8. Lighthouse verification

**Decision**: Pre-merge manual verification by the PR author using Chrome DevTools' Lighthouse panel or `npx @lhci/cli@latest autorun`. Document the threshold (Performance ≥ 90 mobile + desktop) in the PR template / `quickstart.md`. NOT a CI gate for v1.

**Rationale**:
- The site's perf budget is generous for what it ships (one HTML, one CSS, ~7 PNGs); regressions are most likely to come from a maintainer adding a heavy asset, which a pre-merge check catches.
- Adding a CI workflow for Lighthouse adds maintenance surface (Lighthouse CI requires a Lighthouse server or a token-based GitHub App for results storage); not worth it for v1.

**Alternatives considered**:
- **Lighthouse CI as a GitHub Actions workflow**: Reasonable for a future iteration; defer until first regression occurs.
- **No verification at all**: Loses the FR-026 / SC-004 acceptance signal. Rejected.

## R9. Marketing copy voice

**Decision**: Terse, declarative, utility-first — match the existing README voice. Headlines are benefit-oriented but never buzzy.

| Pattern                          | Use? | Example                                            |
|----------------------------------|------|----------------------------------------------------|
| Imperative benefit headline      | YES  | "Know your limits before you hit them."            |
| Stating-a-fact subhead           | YES  | "Live Claude+Codex usage. Analytics. Search."      |
| Buzz-phrase ("AI-powered")       | NO   | —                                                  |
| Exclamations, emojis             | NO   | —                                                  |
| Marketing hyperbole              | NO   | —                                                  |
| Jargon without definition        | NO   | Use "context savings" once, then briefly define.   |

**Rationale**:
- Quill's existing README sets a voice (terse, factual, dense). The marketing site should sound like the same product, not a pivot to a SaaS register.
- Developers (the target audience) actively dislike buzzy copy; concrete and accurate beats grand and vague.

**Alternatives considered**:
- **Punchy SaaS-marketing voice**: Mismatched with the Signal Theater aesthetic and the README baseline. Rejected.

## R10. CI for the Rust env-var override change

**Decision**: Reuse the existing `release.yml` build matrix to catch compile errors on Linux / macOS / Windows. Add one new unit test in `src-tauri/src/data_paths.rs` covering: (a) `QUILL_DEMO_MODE` unset → default path returned, (b) `QUILL_DEMO_MODE=1` + `QUILL_DATA_DIR=/tmp/x` → `/tmp/x` returned, (c) `QUILL_DEMO_MODE=1` + override unset → default path returned with warning.

**Rationale**:
- `release.yml` already builds on all three target platforms on tag pushes; a compile-time regression in `data_paths.rs` would block any release. No new CI workflow needed.
- A unit test on the resolver function is enough to catch logic regressions; integration testing the launcher round-trip is a manual step in the maintainer workflow.

**Alternatives considered**:
- **Add a separate "build on push" CI workflow**: Worth doing for the project independently of this feature; out of scope here.
- **No new test**: Would let a future refactor silently break the demo-mode safety gate. Rejected.

## Summary

All `NEEDS CLARIFICATION` items from the spec's clarify pass are resolved in [spec.md § Clarifications](./spec.md#clarifications). All technical-context unknowns from this plan are resolved above. Phase 1 design proceeds against this baseline.
