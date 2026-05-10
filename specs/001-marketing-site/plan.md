# Implementation Plan: Quill Marketing Site (GitHub Pages)

**Branch**: `001-marketing-site` | **Date**: 2026-05-09 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/001-marketing-site/spec.md`

## Summary

Build a static, single-page marketing site for Quill, hosted at the project's GitHub Pages URL, with a **Terminal Console** visual identity that mirrors the desktop app (dark `#121216` background, monospace headlines, semantic green/yellow/red status pills, ASCII rules). The page deep-links to seven anchored sections (`#hero`, `#live`, `#analytics`, `#context`, `#search`, `#learning`, `#install`). All UI screenshots come from a sandboxed Quill instance pointed at temp directories via two new env vars (`QUILL_DATA_DIR`, `QUILL_RULES_DIR`) gated by an opt-in `QUILL_DEMO_MODE=1` flag, so a maintainer's personal Quill state is never touched. The existing seeder (`scripts/populate_dummy_data.py`) and screenshot driver (`scripts/take_screenshots.sh`) are extended; two new cross-platform launchers wire the sandbox together. A new GitHub Actions workflow (`.github/workflows/pages.yml`) deploys the site via `actions/deploy-pages`.

## Technical Context

**Language/Version**: HTML5 + CSS3 + ES2022 vanilla JS for the site; Rust 2024 edition for the env-var override (existing toolchain); Python 3 for the seeder extension (existing).
**Primary Dependencies**: None for the site (no framework, no build step). GitHub Actions: `actions/checkout@v4`, `actions/configure-pages@v5`, `actions/upload-pages-artifact@v3`, `actions/deploy-pages@v4`. App-side reuses existing crates (`tauri`, `directories` already pulled by Tauri).
**Storage**: N/A for the marketing site (static deliverable). Demo Quill instance writes its SQLite DB to `$QUILL_DATA_DIR/usage.db` instead of the platform default.
**Testing**: Site — manual Lighthouse run (Chrome DevTools or `npx @lhci/cli`) before merge, manual cross-browser smoke (latest Chromium, Firefox, WebKit). App-side — one new unit test in `src-tauri/src/data_paths.rs` covering the env-var resolver under set / unset / demo-mode-off cases. Seeder — manual launcher round-trip on Linux at minimum.
**Target Platform**: GitHub Pages (`https://*.github.io/quill/`) for the site. Demo Quill isolation works on Linux, macOS, and Windows.
**Project Type**: Web (static site, single-page) + small backend changes (Rust path resolver) + scripting (Python seeder flag, two launchers, screenshot driver extension) + one CI workflow.
**Performance Goals**: Lighthouse Performance ≥ 90 on mobile and desktop; Largest Contentful Paint < 2.0 s on simulated broadband; Cumulative Layout Shift < 0.1; total transferred page weight on first load < 500 KB (excluding any optional self-hosted font, kept off for v1 per FR-007 / FR-026).
**Constraints**: Static-only (no server runtime); hero readable with JavaScript disabled (FR-024); honors `prefers-reduced-motion: reduce` (FR-025); WCAG 2.1 AA contrast (FR-008); usable from 320px to 2560px viewport (FR-023); no third-party tracking (FR-028); production builds MUST refuse env-var overrides without `QUILL_DEMO_MODE=1` (FR-018).
**Scale/Scope**: One HTML page, ~7 anchored sections, ~6–10 screenshots, ~150–200 lines HTML + ~300–500 lines CSS + < 100 lines JS for the site. App-side: ~30–50 lines new Rust in `src-tauri/src/data_paths.rs` + ~5–10 call-site updates in `lib.rs`. Scripts: ~30 lines of new Python flag handling + ~40 lines × 2 launcher scripts + minor extension to `take_screenshots.sh`.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

The project's `.specify/memory/constitution.md` is unfilled — every section still contains `[PRINCIPLE_*]` template placeholders and no version has been ratified. There are therefore no ratified gates to evaluate, and the default acceptance criteria apply:

- **Simplicity**: Plan adds one HTML file, one CSS file, one optional ~50-line JS file, one CI workflow, one Rust module, one Python flag set, and two launcher scripts. No new framework, no SSG, no build step, no extra runtime, no extra dependency added to the Tauri app.
- **Scope discipline**: Each artifact maps directly to a numbered FR or to a single locked clarification. No speculative features (no waitlist form, no analytics, no docs site, no localization) are introduced.
- **Reversibility**: All changes can be reverted by deleting `marketing-site/`, `.github/workflows/pages.yml`, `src-tauri/src/data_paths.rs`, the launcher scripts, and the `--data-dir` / `--rules-dir` flags from the seeder. Existing Quill production behavior is unchanged when neither `QUILL_DEMO_MODE` nor any override env var is set.

**Verdict**: PASS (no project-specific gates ratified; baseline simplicity and reversibility checks satisfied).

The Constitution Check is re-evaluated after Phase 1 design (see end of plan).

## Project Structure

### Documentation (this feature)

```text
specs/001-marketing-site/
├── plan.md              # This file
├── spec.md              # Feature spec (already written + clarified)
├── research.md          # Phase 0 output — decisions, rationale, alternatives
├── data-model.md        # Phase 1 output — sandbox layout, asset naming
├── quickstart.md        # Phase 1 output — maintainer walkthrough
├── contracts/           # Phase 1 output — public-surface contracts
│   ├── site-anchors.md
│   ├── env-vars.md
│   ├── seeder-cli.md
│   ├── launcher-cli.md
│   └── pages-workflow.md
├── checklists/
│   └── requirements.md  # Already written by /speckit-specify
└── tasks.md             # Phase 2 output — created by /speckit-tasks (not this command)
```

### Source Code (repository root)

```text
marketing-site/                              # NEW — site source root (FR-002)
├── index.html                               # Single page with seven anchored sections
├── styles.css                               # Terminal Console theme; mono stack
├── scripts.js                               # Optional, ≤ 5 KB micro-animation only
├── assets/
│   ├── screenshots/                         # @2x PNG captures from sandboxed Quill
│   │   ├── hero.png
│   │   ├── live.png
│   │   ├── analytics-now.png
│   │   ├── analytics-charts.png
│   │   ├── analytics-context.png
│   │   ├── sessions.png
│   │   └── learning.png
│   ├── og-image.png                         # 1200x630 social-share preview
│   └── favicon.svg
└── README.md                                # Source-tree map and contribution notes

.github/workflows/
└── pages.yml                                # NEW — Actions workflow that publishes (FR-003)

src-tauri/src/
├── data_paths.rs                            # NEW — env-var path resolver, opt-in via QUILL_DEMO_MODE (FR-018)
└── lib.rs                                   # MODIFIED — call resolver instead of bare app_data_dir() / hard-coded learned-rules dirs

scripts/
├── populate_dummy_data.py                   # MODIFIED — accepts --data-dir / --rules-dir (FR-018)
├── take_screenshots.sh                      # MODIFIED — captures additional views (Context tab, Settings)
├── run_quill_demo.sh                        # NEW — POSIX launcher (Linux + macOS) (FR-018)
└── run_quill_demo.ps1                       # NEW — Windows PowerShell launcher (FR-018)

CLAUDE.md                                    # MODIFIED — SPECKIT block points at this plan
```

**Structure Decision**: Single static-page web deliverable (`marketing-site/`) plus a thin app-side path-isolation seam (`src-tauri/src/data_paths.rs`) and the launcher/seeder script trio (`scripts/run_quill_demo.*`, `scripts/populate_dummy_data.py`). No build step, no SSG, no framework. Existing screenshot driver (`scripts/take_screenshots.sh`) is extended rather than replaced. The `marketing-site/` directory is the sole source root for the site and the only path the Pages workflow uploads as the Pages artifact, so source layout and deploy contract are 1:1.

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No constitution gate violations. Section intentionally empty.

## Phase 0 Research Summary

See [research.md](./research.md) for full Decision / Rationale / Alternatives entries. Topics resolved:

1. Static plain-HTML/CSS vs static-site-generator → **plain HTML**
2. Typography (FR-007 forbids Inter) → **system monospace stack**
3. Rust env-var override pattern → **dedicated `data_paths.rs` module, opt-in via `QUILL_DEMO_MODE=1`**
4. Cross-platform launcher shape → **`.sh` + `.ps1` pair, no Python launcher**
5. Screenshot scope → **7 captures matching the seven anchored sections**
6. GitHub Pages workflow shape → **two-job `pages.yml` using official `actions/deploy-pages`**
7. OG / social-share image → **hand-built 1200×630 PNG, hero-derived**
8. Lighthouse verification → **manual pre-merge run for v1 (no CI gate yet)**
9. Marketing copy voice → **terse, declarative, README-aligned (no buzzy SaaS register)**
10. CI for Rust path resolver change → **rely on existing `release.yml` build matrix; add one new unit test**

## Phase 1 Design Summary

See [data-model.md](./data-model.md), [contracts/](./contracts/), and [quickstart.md](./quickstart.md). Phase 1 produced:

- **Sandbox layout** (`data-model.md`): the temp-dir tree the launcher creates, the screenshot asset naming convention, and the site source-file conventions.
- **Site-anchor contract** (`contracts/site-anchors.md`): the seven anchor IDs and their semantic meaning, declared as public deep-link surface.
- **Env-var contract** (`contracts/env-vars.md`): `QUILL_DEMO_MODE`, `QUILL_DATA_DIR`, `QUILL_RULES_DIR` — gating, precedence, error behavior.
- **Seeder CLI contract** (`contracts/seeder-cli.md`): `populate_dummy_data.py` flag surface and exit codes after extension.
- **Launcher CLI contract** (`contracts/launcher-cli.md`): `run_quill_demo.sh` / `.ps1` arguments, environment, lifecycle.
- **Pages workflow contract** (`contracts/pages-workflow.md`): triggers, paths filter, permissions, concurrency, jobs.
- **Quickstart** (`quickstart.md`): the maintainer walkthrough — clone, install Quill, run launcher, capture, preview locally, commit, ship.

## Constitution Check (post-Phase-1 re-evaluation)

Re-evaluated after Phase 1 design.

- **Simplicity preserved**: Phase 1 introduced no extra runtimes, frameworks, or services. Contracts are short markdown documents; the data model is a directory tree; the quickstart is shell-and-keyboard maintenance instructions.
- **Scope preserved**: Every Phase-1 artifact maps to a clarified spec decision. No accidental scope expansion (e.g., no signup form contract, no analytics contract, no localization).
- **Reversibility preserved**: All Phase-1 deliverables are documents. None of them prescribe irreversible runtime behavior.

**Verdict**: PASS.

## Stop Conditions

Plan command ends after Phase 2 planning is implicitly defined (the structure tasks would take in `tasks.md`). The actual `tasks.md` is generated by `/speckit-tasks`, NOT by this command. No further work happens here.
