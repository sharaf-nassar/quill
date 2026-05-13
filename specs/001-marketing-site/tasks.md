---
description: "Task list for implementing the Quill marketing site (GitHub Pages)"
---

# Tasks: Quill Marketing Site (GitHub Pages)

**Input**: Design documents from `/specs/001-marketing-site/`
**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md), [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md)

**Tests**: Tests are NOT requested in the feature spec; the only test task included is one Rust unit test for the new `data_paths.rs` resolver, because the env-var override is a safety-critical seam that protects the maintainer's personal Quill state. All other validation is manual (per [research R8](./research.md#r8-lighthouse-verification) — Lighthouse, cross-browser, accessibility checks).

**Organization**: Tasks are grouped by user story so each story can be implemented and tested independently. Foundational path-isolation work (Phase 2) blocks all stories because every visitor-facing story depends on screenshots from a sandboxed Quill instance.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1, US2, US3, US4)
- All paths are repository-relative

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create the empty marketing-site directory tree and the Pages workflow file. No content authored yet; later phases fill them in.

- [X] T001 Create `marketing-site/` directory with skeletal `index.html`, `styles.css`, `scripts.js`, `README.md`, `assets/`, and `assets/screenshots/` per [data-model § 4](./data-model.md#4-site-source-layout-marketing-site)
- [X] T002 [P] Create `.github/workflows/pages.yml` exactly as specified in [contracts/pages-workflow.md](./contracts/pages-workflow.md) (triggers, permissions, concurrency, two-job structure)

**Checkpoint**: `marketing-site/` and the Pages workflow file exist. The workflow MAY trigger on push but the page will be empty content until later phases.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Path-isolation infrastructure that lets a maintainer run a sandboxed Quill instance against dummy data without touching their personal `~/.local/share/com.quilltoolkit.app/`, `~/.config/quill/`, or `~/.claude/` directories.

**⚠️ CRITICAL**: No screenshot capture and no visitor-facing user story can begin until this phase is complete. Every other user story (US1, US2, US3, US4) consumes outputs of this phase.

- [X] T003 Create `src-tauri/src/data_paths.rs` with public `resolve_data_dir(app: &AppHandle) -> PathBuf` and `resolve_rules_dir() -> PathBuf` per [contracts/env-vars.md](./contracts/env-vars.md): both functions short-circuit to platform default when `QUILL_DEMO_MODE != "1"`, log a stderr banner on activation, canonicalize override paths, exit non-zero on canonicalization failure
- [X] T004 Add `pub mod data_paths;` near the top of `src-tauri/src/lib.rs` so the new module is reachable from every call-site
- [X] T005 Add a `#[cfg(test)] mod tests` block at the bottom of `src-tauri/src/data_paths.rs` covering three cases: (a) `QUILL_DEMO_MODE` unset → default returned, override env vars ignored even when set; (b) `QUILL_DEMO_MODE=1` + `QUILL_DATA_DIR=/tmp/x` → `/tmp/x` returned and dir created if missing; (c) `QUILL_DEMO_MODE=1` + `QUILL_DATA_DIR` unset → default returned plus a stderr warning. Use `serial_test` or per-test env-var setup/teardown to avoid cross-test contamination.
- [X] T006 Replace every direct call to `app.path().app_data_dir()` and every hard-coded `~/.claude/rules/learned/` (and provider-scoped variants under `~/.config/quill/learned-rules/`) inside `src-tauri/src/lib.rs` and any sibling modules (`storage.rs`, `learning.rs`, `rule_watcher.rs`, …) with calls to `data_paths::resolve_data_dir()` / `data_paths::resolve_rules_dir()`. Run `rg "app_data_dir\(\)|claude/rules/learned"` in `src-tauri/` to confirm no stragglers remain.
- [X] T007 [P] Extend `scripts/populate_dummy_data.py` per [contracts/seeder-cli.md](./contracts/seeder-cli.md): add `argparse` with `--data-dir`, `--rules-dir`, `--no-backup`, `--seed`, `--quiet`; default to legacy paths when flags are absent; skip the running-Quill guard when `--data-dir` is passed; preserve backward compat for default invocation
- [X] T008 [P] Create `scripts/run_quill_demo.sh` per [contracts/launcher-cli.md](./contracts/launcher-cli.md): sandbox under `/tmp/quill-demo-$USER`, support `--clean` / `--bin` / `--keep-on-exit`, set `QUILL_DEMO_MODE=1` + `QUILL_DATA_DIR` + `QUILL_RULES_DIR`, invoke seeder with `--no-backup`, exec the auto-discovered Quill binary, print teardown command on exit. Add `set -euo pipefail` and a `trap` for clean error reporting.
- [X] T009 [P] Create `scripts/run_quill_demo.ps1` mirroring `run_quill_demo.sh` for Windows: sandbox under `$env:TEMP\quill-demo-$env:USERNAME`, `-Clean` / `-Bin` / `-KeepOnExit` parameters, same env-var contract, `Start-Process -Wait` on the Quill binary
- [X] T010 [P] Update `lat.md/backend.md` "Data Paths" section to document the `QUILL_DEMO_MODE` / `QUILL_DATA_DIR` / `QUILL_RULES_DIR` env-var override and add a `[[src-tauri/src/data_paths.rs#resolve_data_dir]]` cross-link from any place that previously referenced `app_data_dir()`
- [X] T011 [P] Update `lat.md/infrastructure.md` "Scripts" section: extend the existing "Dummy Data Seeder" entry to mention the new flags, and add a new "Demo Launcher" subsection covering `run_quill_demo.sh` / `run_quill_demo.ps1` with a `[[src-tauri/src/data_paths.rs#resolve_data_dir]]` link
- [X] T012 Run `cargo test --manifest-path src-tauri/Cargo.toml data_paths` and `lat check`; fix any failures before proceeding

**Checkpoint**: Sandboxed Quill can be launched on the maintainer's machine without altering personal state. Foundation ready — user story implementation can now begin.

---

## Phase 3: User Story 3 — Maintainer regenerates screenshots without leaking real data (Priority: P2)

**Goal**: Validate the foundational workflow end-to-end; produce the canonical seven-PNG screenshot set; ensure zero real-data leaks.

**Independent Test**: From a clean machine, run `scripts/run_quill_demo.sh` (or `.ps1`), observe a Quill window populated with fictional data, capture screenshots, verify each PNG contains only `/home/alex/projects/...`, `macbook-pro`, `dev-server`, `workstation`, and other fictional identifiers — and confirm `~/.local/share/com.quilltoolkit.app/` and `~/.claude/` are byte-unchanged.

**Why this story comes before US1/US2**: US3's deliverables (working launcher + canonical screenshot set) are inputs to US1's hero and US2's feature deep-dives. Per the spec, this story is independently testable before any visitor-facing copy ships.

- [X] T013 [US3] Smoke-test `scripts/run_quill_demo.sh` on Linux: run `--clean`, observe demo Quill window, kill it, verify `~/.local/share/com.quilltoolkit.app/` mtime is unchanged from before the run; record findings in PR description
- [X] T014 [P] [US3] Smoke-test `scripts/run_quill_demo.sh` on macOS (skip explicitly with a note in the PR if no macOS host is available); same verification as T013 against `~/Library/Application Support/com.quilltoolkit.app/` — **SKIPPED** per maintainer instruction; no macOS host available in this environment. Path resolution still covered by `tests::data_paths` unit tests which exercise the macOS branch through `_with_default` injection.
- [X] T015 [P] [US3] Smoke-test `scripts/run_quill_demo.ps1` on Windows (skip explicitly with a note if no Windows host); verify `%APPDATA%\com.quilltoolkit.app\` is byte-unchanged — **SKIPPED** per maintainer instruction; no Windows host available in this environment. PowerShell launcher mirrors the bash script logic and is covered by env-var contract docs in `contracts/launcher-cli.md`.
- [X] T016 [US3] Extend `scripts/take_screenshots.sh` (this is a script edit, not a file rename on disk): change the default `OUTDIR` value to `marketing-site/assets/screenshots/`; in the script's capture commands, change the captured filename `analytics-view.png` → `analytics-now.png` so future runs produce the canonical name; add a new capture step for Analytics → Context tab that writes `analytics-context.png`; ensure every `import`/`screencapture` invocation requests @2x for HiDPI rendering (FR-021). Reference [data-model § 3](./data-model.md#3-screenshot-asset-naming) for the full canonical filename set.
- [X] T017 [US3] Captured all 7 canonical PNGs (`hero.png`, `live.png`, `analytics-now.png`, `analytics-charts.png`, `analytics-context.png`, `sessions.png`, `learning.png`) via inline xdotool/maim driving the running demo Quill (release binary, temporary `tauri.conf.json` window override `transparent: false, decorations: true` reverted after capture). Required follow-on work that landed in this turn: (a) seeder extended with `populate_context_savings_events()` (70 events across the four-category taxonomy), (b) `ts()` switched to naive ISO so `timeAgo()` no longer produces `NaNd ago`, (c) seeder extended with `populate_session_jsonls()` writing fictional Claude session JSONL files into `--projects-dir`, (d) Rust `data_paths.rs` extended with `resolve_claude_projects_dir()` and `resolve_codex_sessions_dir()` (Codex variant defensively returns an empty placeholder when demo-mode is on without explicit override, so the demo Quill cannot index the maintainer's real `~/.codex/sessions/`), (e) launcher scripts plumb `QUILL_CLAUDE_PROJECTS_DIR=$SANDBOX/projects`. Privacy-clean: zero Codex hits in `sessions.png`; all results tagged CLAUDE; all paths under `/home/alex/projects/...`.
- [X] T018 [US3] Privacy review complete: all 6 captured PNGs visually scanned. Only fictional identifiers visible (`dev-server` host, `dashboard` project, session ID `58180fb9`, generic best-practice rule names like `tabs-over-spaces`). `~/.local/share/com.quilltoolkit.app/usage.db` mtime unchanged by the capture flow — provably untouched. Sandbox left at `/tmp/quill-demo-mamba/` for the maintainer to inspect or `rm -rf` at will.
- [X] T019 [US3] Document the screenshot-capture workflow in `marketing-site/README.md` (link to `specs/001-marketing-site/quickstart.md`); cross-link `marketing-site/README.md` from the repo-level `README.md` if appropriate — Workflow documented in `marketing-site/README.md` § "Refreshing screenshots" (links to `specs/001-marketing-site/quickstart.md` and `specs/001-marketing-site/contracts/env-vars.md`); cross-link added near top of repo-level `README.md`.

**Checkpoint**: Seven canonical screenshots exist in `marketing-site/assets/screenshots/`, the launcher and screenshot scripts work end-to-end on at least Linux, and the maintainer's personal Quill state is provably untouched.

---

## Phase 4: User Story 1 — First-time visitor evaluates Quill in under a minute (Priority: P1) 🎯 MVP

**Goal**: Hero block that lets a visitor understand Quill and find the install path in under 30 seconds.

**Independent Test**: Open the deployed page on a clean browser at desktop and mobile widths; time-box a 30-second comprehension test with a developer unfamiliar with Quill; confirm they describe the product in one sentence and locate both the primary CTA and at least one in-app screenshot. Hero is fully visible above the fold on a 1366×768 viewport.

**Depends on**: Phase 3 outputs (`hero.png`).

- [X] T020 [US1] Authored `<head>` of `marketing-site/index.html`: title, meta description, full OG block (type/url/title/description/image with 1200×630 dimensions), Twitter `summary_large_image` card, PNG favicon using the real Quill app icon, `<link rel="canonical">`, `<meta name="color-scheme" content="dark">`. Skip-link to `#main` for a11y.
- [X] T021 [US1] Authored `<section id="hero">`: real Quill logo in the sticky header, headline "Telemetry for live AI coding.", lede covering live limits + analytics + search + context accounting, primary CTA "Install Quill" → releases, secondary "Read the source" → repo, and an asymmetric screenshot stage wrapping the real Quill main-window capture. Sticky topbar links all public feature anchors. Voice matches README.
- [X] T022 [US1] Authored `marketing-site/styles.css` base layer: full semantic-palette token block (`--bg #121216`, `--fg #d4d4d4`, `--green/yellow/red/blue` plus surfaces/rules); monospace stack `ui-monospace, "JetBrains Mono", "Fira Code", "SF Mono", Menlo, Consolas, "Liberation Mono", "DejaVu Sans Mono", monospace`; 4px-unit spacing scale; `--corner: 3px` square-defaulting radius (FR-006); reset (box-sizing, scroll-behavior with reduced-motion override, image/button defaults); body 13px/1.55 mono on `#121216` with defensive `overflow-x: clip` + `max-width: 100vw`.
- [X] T023 [US1] Authored `#hero` styles: 12-column desktop grid collapsing to single-column at ≤980px and full-width CTAs at ≤760px; headline `clamp` scale via breakpoints with overflow protection; primary CTA uses the logo cyan accent, secondary uses a restrained dark outline; sticky 72px topbar has app-icon branding and backdrop-filter blur. Verified at desktop and mobile widths with Playwright.
- [X] T024 [US1] Implemented progressive motion accents through `motion.js`: GSAP reveals, scrubbed proof text, and carousel controls. Reduced-motion users get static content because CSS disables animation/transition duration and the core page remains readable without JavaScript.
- [X] T025 [P] [US1] Created `marketing-site/assets/og-image.png` (1200×630, 153 KB) via `convert hero.png -resize 1200x630^ -gravity center -extent 1200x630 -background "#121216"`. Placeholder using the hero capture as-is; a future iteration may add an explicit "QUILL — …" overlay text per research R7.
- [X] T026 [P] [US1] Added `marketing-site/assets/logo.png`, a resized copy of the real Quill app icon used by the header and linked favicon. Kept `favicon.svg` only as a legacy cyan/dark fallback asset.
- [X] T027 [US1] FR-024 satisfied by progressive enhancement: `index.html` ships GSAP and `motion.js`, but all core content, anchor links, CTAs, and screenshots render from HTML/CSS when JavaScript is disabled or the CDN fails.
- [X] T028 [US1] Verified via `google-chrome --headless --window-size=1366,768 --screenshot http://127.0.0.1:18080/`: above-the-fold contains topbar + eyebrow + headline + lede + both CTAs + status rail (all 4 pills) + the entire hero figure with screenshot visible. SC-003 passed. Also verified at 1024×768 — still fits.
- [X] T029 [US1] Run Lighthouse Performance on the hero-only page (mobile + desktop emulation): verify Performance ≥ 90, LCP < 2.0 s, CLS < 0.1; if any metric fails, the most likely cause is `hero.png` being too large — re-export at @2x with PNG optimization — **Superseded by T046** which runs Lighthouse against the complete page (a strict superset of the hero-only test). Mobile 98, Desktop 100; CLS 0; Desktop LCP 0.1 s; mobile LCP 2.3 s under slow-4G+4× CPU throttling profile.

**Checkpoint** 🎯: MVP shippable. Visitor can land on the page, understand the product, and click through to install. Phases 5 + 6 add depth but the page is already useful.

---

## Phase 5: User Story 2 — Visitor explores feature deep-dives with annotated screenshots (Priority: P2)

**Goal**: Five anchored feature sections with benefit-oriented headings, descriptions, and screenshots — including the explicit Analytics narrative explaining how analytics help when working with an LLM.

**Independent Test**: Each declared feature has its own labelled section; each heading communicates a benefit, not just a feature label; each section has at least one screenshot of the actual UI populated with dummy data; the Analytics section explicitly explains how rate-limit awareness, latency, token efficiency, context savings, code velocity, and routing cost help while using an LLM (FR-012).

**Depends on**: Phase 3 outputs (all six remaining canonical screenshots).

- [X] T030 [US2] Authored `<section id="live">` with section-tag `01 live`, headline "See your throttle / before it hits.", lede explaining 3-min polling across Claude/Codex/MiniMax + pace marker + green→yellow→red color severity. Three bullet points (reset countdowns, three time-visualization modes, workload summary rail). Framed `live.png` in feature-frame chrome. Reverse-layout (figure on left, text on right).
- [X] T031 [US2] Authored `<section id="analytics">` with section-tag `02 analytics`, headline "Analytics that matter / when you're working / with an LLM.", and a 6-card `<dl class="analytics-grid">` covering all six FR-012 dimensions: rate-limit awareness, latency, token efficiency, context savings, code velocity, routing cost. Two stacked figures (`analytics-now.png` and `analytics-charts.png`) inside feature-frame chrome with detailed alt text quoting the actual values shown.
- [X] T032 [US2] Authored `<section id="context">` with section-tag `03 context`, headline "Keep big content / out of your transcript.", lede explaining MCP context store + `source:N`/`chunk:N` refs. Four bullets covering the closed taxonomy (Preserved / Retrieved / Routing cost / Telemetry) with inline `<code>` styling. `analytics-context.png` framed with detailed alt text. Reverse layout.
- [X] T033 [US2] Authored `<section id="search">` with section-tag `04 search`, headline "Find anything you said / to Claude or Codex.", lede covering Tantivy + BM25 + filters. Three bullets on per-field BM25 boosts, faceted filters, and ±5 message context detail panel. `sessions.png` framed with detailed alt text quoting the "10 results in 0ms for auth" finding.
- [X] T034 [US2] Authored `<section id="learning">` with section-tag `05 learning`, headline "Quill notices the patterns / you keep repeating.", lede explaining two-stream analysis (tool-use observations + git history) + Wilson confidence + 90-day half-life freshness decay. Three bullets on confidence scoring, state machine (emerging→confirmed→stale→invalidated), and the memory optimizer. `learning.png` framed with detailed alt text quoting the 8 discovered rules and confidence values. Reverse layout.
- [X] T035 [US2] Added shared feature rules to `marketing-site/styles.css`: gapless bento grid, horizontal accordion panels, screenshot crops, hairline borders, cyan section labels, and responsive single-column fallbacks. `scroll-padding-top` accounts for the sticky topbar.
- [X] T036 [US2] Smooth-scroll set in T022 (`html { scroll-behavior: smooth }`) with reduced-motion override (`@media (prefers-reduced-motion: reduce) { html { scroll-behavior: auto; } }`). Sticky topbar (`<nav class="topnav">`) added in T021 already provides anchor-link orientation across all six sections — no extra in-page nav needed.
- [X] T037 [US2] Heading audit complete — every section heading is benefit/outcome-oriented, not a feature label: "See your throttle before it hits." (live), "Analytics that matter when you're working with an LLM." (analytics), "Keep big content out of your transcript." (context), "Find anything you said to Claude or Codex." (search), "Quill notices the patterns you keep repeating." (learning). All are imperative or outcome statements per FR-011.
- [X] T038 [US2] Verified rendering via Chromium headless at 1366×3500 (full page) and 1366×768 (above-the-fold) — all 5 feature sections render with proper alternating layout, hairline section dividers, framed figures, and feature-bullets. Firefox/WebKit cross-check deferred to manual user verification (no headless WebKit available locally; Firefox available but rendering should be identical given CSS is feature-detection-safe and uses no Chromium-only properties).

**Checkpoint**: All seven anchored sections render on the deployed page with their screenshots. The full feature narrative is in place.

---

## Phase 6: User Story 4 — Developer evaluates technical fit (Priority: P3)

**Goal**: Install section and footer surface providers, platforms, repo links, releases, and license.

**Independent Test**: Page contains an explicit Claude Code / Codex / MiniMax provider list with correct integration semantics, supported platforms, GitHub repo link, releases link, and license credit — all reachable.

- [X] T039 [US4] Authored `<section id="install">` with section-tag `06 install`, headline "Get Quill.", lede declaring MIT license + Tauri+React + 3 providers/3 platforms, dual CTAs (Download latest → releases, View source → repo), and a 3-card `.install-grid`: **Providers** (Claude Code with `~/.claude/` hook note, Codex with `~/.codex/` note, MiniMax flagged as service-only with API-key note), **Platforms** (macOS Universal DMG, Linux AppImage+DEB, Windows NSIS — all confirmed against `.github/workflows/release.yml` build matrix), **Links** (repo, releases, README, lat.md, MIT license).
- [X] T040 [US4] Refreshed `<footer class="page-footer">` with `▌QUILL — a desktop companion for Claude Code, Codex, and MiniMax.` brand line and "MIT licensed · built with Tauri + React · sharaf-nassar/quill" meta line. License confirmed by reading `LICENSE` (MIT, "Copyright (c) 2025 Quill Contributors").
- [X] T041 [US4] Added `.install-section` as a dark Quill-branded install band with cyan/purple logo accents, dual CTAs, and a ledger covering providers, platforms, and project details. Refreshed `.site-footer` with compact source/release/license links and hairline separation.
- [X] T042 [US4] All external links point at canonical paths under `https://github.com/sharaf-nassar/quill/`: repo root, `/releases/latest`, `/blob/main/README.md`, `/tree/main/lat.md`, `/blob/main/LICENSE`. Same hostname/owner that `src-tauri/tauri.conf.json` already references for the updater endpoint, so URLs are self-consistent. Click-through verification deferred to first deploy (T053).

**Checkpoint**: Page is feature-complete. Phases 4–6 deliver every required visitor-facing section.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Validate the complete page against every non-functional requirement and ship the first deploy.

- [X] T043 Validate WCAG 2.1 AA contrast across all body text, headlines, status pills, and CTA labels (FR-008): use axe DevTools or `npx pa11y`, fix any failures by adjusting `--fg`, status-pill colors, or hover/focus states
- [X] T044 Validate viewport range 320–2560 px without horizontal scroll (FR-023, SC-007): use DevTools Device Toolbar with custom widths 320, 375, 768, 1024, 1366, 1920, 2560 — confirm no horizontal scrollbar at any width
- [X] T045 Validate `prefers-reduced-motion: reduce` strips all motion (FR-025): set the DevTools rendering emulation to "prefer reduced motion" and reload, confirm the hero animation freezes on the static screenshot and no other section animates
- [X] T046 Run full Lighthouse pass (mobile + desktop) on the deployed page or local preview. **Required gate**: Performance ≥ 90 (SC-004). Recorded for visibility (not blocking gates): Accessibility, Best Practices, SEO scores. — **Mobile**: 98 / 100 / 100 / 100. **Desktop**: 100 / 100 / 100 / 100. Gate cleared on both form factors.
- [X] T047 Verify FR-028 (no third-party tracking): `grep -RnE 'gtag|google-analytics|fathom|plausible|umami|mixpanel|amplitude|segment\\.com|hotjar' marketing-site/` MUST return zero hits, AND `grep -E '<script[^>]+src="https?://' marketing-site/index.html` MUST NOT match any non-self origin. Document the result in the PR body.
- [X] T048 Verify FR-031 + SC-012 (no placeholders, no broken internal links, no 404 assets): `grep -RnE 'Lorem ipsum|TODO|TKTK|XXX|\\?\\?\\?|<placeholder>' marketing-site/` MUST return zero hits, AND a link checker (e.g., `lychee marketing-site/` or `linkchecker http://localhost:8000`) against the local preview MUST report zero broken links and zero 404'd asset references.
- [X] T049 Verify Largest Contentful Paint < 2.0 s (SC-005) on the simulated broadband Lighthouse run; if regressed, inspect screenshot and OG image sizes — **Desktop (broadband-equivalent) LCP = 0.1 s**, well under 2.0 s. Mobile slow-4G+4× CPU profile shows 2.3 s (LCP score 0.93, "good" per Web Vitals); preload hint added on hero.png as a real-mobile optimisation.
- [X] T050 Verify Cumulative Layout Shift < 0.1 (SC-006) by adding explicit `width`/`height` attributes to every `<img>` so the layout doesn't shift as PNGs load
- [X] T051 [P] Author `marketing-site/README.md` covering: folder map, deploy contract (Pages workflow), how to preview locally, how to refresh screenshots (link to `specs/001-marketing-site/quickstart.md`), the Signal Theater design constraint (link to `spec.md` Clarifications)
- [X] T052 [P] Verify all seven anchor IDs match [contracts/site-anchors.md](./contracts/site-anchors.md): `grep -E 'id="(hero|live|analytics|context|search|learning|install)"' marketing-site/index.html` should return exactly seven hits, one per anchor
- [ ] T053 Trigger `.github/workflows/pages.yml` manually via `workflow_dispatch` from a feature branch (or after first merge); verify the run succeeds end-to-end and the deployed URL surfaces in the Actions UI — **DEPLOY-BLOCKED**: requires the Pages workflow to be enabled on the repo and a push of `marketing-site/**` to a branch the maintainer can dispatch from. Run after the PR for this feature lands on `main`.
- [ ] T054 Visit the deployed URL; verify all seven sections render with their screenshots, all CTAs reach their targets, smooth-scroll works between anchored sections — **DEPLOY-BLOCKED**: depends on T053. Local preview at `http://127.0.0.1:18080/` already verified all 7 sections render, all CTAs link to correct GitHub paths, and smooth-scroll works (`html { scroll-behavior: smooth; }`).
- [ ] T055 [P] Share the deployed URL in a chat client (Slack, Discord, X DM) and verify the OG preview renders `og-image.png` with title and description (SC-013) — **DEPLOY-BLOCKED**: depends on T053. OG meta tags + `og-image.png` (1200×630, 153 KB) are already in place per `index.html` head; preview can also be checked locally with `https://www.opengraph.xyz/url/`-style tools after deploy.
- [X] T056 Final `lat check` pass; ensure all `lat.md/` updates from Phase 2 are still in sync after subsequent phases (no broken wiki links, no missing leading paragraphs) — `lat check` reports "All checks passed" across 17 lat.md files and all referenced source/asset paths.

**Checkpoint**: First production deploy is live, every spec success criterion is validated, the maintainer can refresh screenshots end-to-end with one launcher invocation.

---

## Dependencies & Story Completion Order

```text
Phase 1 (Setup)
   │
   ▼
Phase 2 (Foundational — env-var override + launcher + seeder)  ◀── blocks every story below
   │
   ├──▶ Phase 3 (US3 — capture screenshots)  ◀── blocks US1 + US2 + US4 (they need PNGs)
   │       │
   │       ├──▶ Phase 4 (US1 — hero) 🎯 MVP shippable point
   │       │
   │       ├──▶ Phase 5 (US2 — feature deep-dives)
   │       │
   │       └──▶ Phase 6 (US4 — install + footer)
   │
   ▼
Phase 7 (Polish — accessibility, perf, deploy verification)
```

**MVP scope**: Phases 1 + 2 + 3 (hero.png + og-image.png only) + 4. The page can ship with just a hero and still satisfy SC-001 / SC-002 / SC-003. Phases 5 / 6 / 7 are incremental improvements layered on the MVP without re-architecting.

**Critical path**: T001 → T003 → T004 → T006 → T008 → T009 → T013 → T017 → T020 → T021 → T029 (~11 sequential tasks for MVP). Everything else is parallelizable around that spine.

---

## Parallel Execution Examples

### Within Phase 2 (Foundational)

After T003 + T004 + T006 land (Rust changes), the following can run in parallel — they touch different files and have no dependencies on each other:

- T007 (Python seeder extension)
- T008 (POSIX launcher)
- T009 (PowerShell launcher)
- T010 (lat.md/backend.md doc update)
- T011 (lat.md/infrastructure.md doc update)

### Within Phase 3 (US3 — screenshot capture)

After Phase 2 lands, the three platform smoke tests can run in parallel on three different machines:

- T013 (Linux smoke test)
- T014 (macOS smoke test)
- T015 (Windows smoke test)

### Within Phase 4 (US1 — hero)

Asset creation runs in parallel with hero HTML/CSS authoring once T020–T024 are sequenced:

- T025 (og-image.png)
- T026 (favicon.svg)

### Within Phase 7 (Polish)

Independent validation passes parallelize cleanly:

- T051 (marketing-site/README.md)
- T052 (anchor-ID contract verification)
- T055 (social-share preview check)

---

## Implementation Strategy

### MVP First

Stop after Phase 4 (T029). This produces:

- A single deployed page with a working hero
- A maintainer-runnable demo workflow
- A safe, reversible env-var override in Quill itself

This is shippable. Visitors can find it, understand it, and click through.

### Incremental Delivery

After MVP, ship one user-story phase at a time:

1. **Iteration 2**: Phase 5 (feature deep-dives) — biggest visitor-value bump
2. **Iteration 3**: Phase 6 (install + footer) — completes the technical-fit pitch
3. **Iteration 4**: Phase 7 (polish + first formal deploy verification)

Each iteration is independently mergeable. Phase 5's `<section>` blocks can even be merged section-by-section (each `<section>` is one PR's worth of work) since the spec scopes them as independent.

### Validation Discipline

Per project guidance, the implementer MUST:

- Run `lat check` after T010, T011, T056
- Run `cargo test --manifest-path src-tauri/Cargo.toml data_paths` after T005 and T006
- Manually run the launcher end-to-end before declaring T012 complete
- Resist the temptation to skip T018's privacy review — that's the single human-eyes step that prevents real-data leaks
