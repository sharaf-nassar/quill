# Implementation Plan: Quill Marketing Site (GitHub Pages)

**Branch**: `001-marketing-site` | **Date**: 2026-05-09 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/001-marketing-site/spec.md`

## Summary

Build a static, single-page marketing site for Quill with a **Signal Theater** identity, stable anchors, progressive motion, and real app screenshots as primary proof. Canonical screenshots now render the actual React entry point in dev-only Browser Mock Mode inside network-disabled Docker and are driven through headless Chromium. The sandboxed Tauri launcher and SQLite seeder remain backend-development tools. GitHub Actions deploys the static site via `actions/deploy-pages`.

## Technical Context

**Language/Version**: HTML5 + CSS3 + small progressive JavaScript for the site; Rust 2024 edition for the env-var override (existing toolchain); Python 3 for the seeder extension (existing).
**Primary Dependencies**: Browser-native IntersectionObserver and CSS transitions for progressive marketing-page motion; no framework, runtime dependency, or build step. GitHub Actions: `actions/checkout@v4`, `actions/configure-pages@v5`, `actions/upload-pages-artifact@v3`, `actions/deploy-pages@v4`. App-side reuses existing crates (`tauri`, `directories` already pulled by Tauri).
**Storage**: N/A for the site or canonical screenshot runtime; marketing fixtures are in-memory Browser Mock Mode data.
**Testing**: Project gates, Docker capture round-trip, PNG dimension checks, responsive/cross-browser review, and Lighthouse before merge.
**Target Platform**: GitHub Pages for the site; Linux Docker + Chromium for canonical capture.
**Project Type**: Static site + dev-only frontend fixture profile + bounded CDP capture script + Pages workflow.
**Performance Goals**: Lighthouse Performance ≥ 90 on mobile and desktop; Largest Contentful Paint < 2.0 s on simulated broadband; Cumulative Layout Shift < 0.1; total transferred page weight on first load < 500 KB (excluding any optional self-hosted font, kept off for v1 per FR-007 / FR-026).
**Constraints**: Static-only site; actual app components only; Browser Mock Mode remains dev-only; capture has no host state or external network; hero works without JavaScript; reduced motion, WCAG AA, 320–2560px responsiveness, and no tracking.
**Scale/Scope**: One HTML page, eleven anchors, nine screenshots, one existing mock fixture module, and one CDP capture script.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

The project's `.specify/memory/constitution.md` is unfilled — every section still contains `[PRINCIPLE_*]` template placeholders and no version has been ratified. There are therefore no ratified gates to evaluate, and the default acceptance criteria apply:

- **Simplicity**: The site stays plain HTML/CSS/JS. Capture reuses Vite, Chromium, and the installed Tauri API mocks; no browser automation dependency or screenshot-only UI is added.
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
└── contracts/           # Phase 1 output — public-surface contracts
    ├── site-anchors.md
    ├── env-vars.md
    ├── seeder-cli.md
    ├── launcher-cli.md
    └── pages-workflow.md
```

### Source Code (repository root)

```text
marketing-site/                              # NEW — site source root (FR-002)
├── index.html                               # Single page with eleven anchored sections
├── styles.css                               # Signal Theater theme; no remote fonts
├── motion.js                                # Progressive native scroll-reveal behavior
├── assets/
│   ├── screenshots/                         # @2x Browser Mock Mode captures
│   │   ├── hero.png
│   │   ├── live.png
│   │   ├── models.png
│   │   ├── analytics-context.png
│   │   ├── sessions.png
│   │   ├── learning.png
│   │   ├── settings.png
│   │   ├── memory.png
│   │   └── brevity.png
└── README.md                                # Source-tree map and contribution notes

.github/workflows/
└── pages.yml                                # NEW — Actions workflow that publishes (FR-003)

src-tauri/src/
├── data_paths.rs                            # NEW — env-var path resolver, opt-in via QUILL_DEMO_MODE (FR-018)
└── lib.rs                                   # MODIFIED — call resolver instead of bare app_data_dir() / hard-coded learned-rules dirs

scripts/
├── capture_browser_screenshots.mjs          # CDP driver for the actual frontend
├── capture_screenshots_docker.sh            # isolated publishing wrapper
├── populate_dummy_data.py                   # backend/Tauri fixture tool
├── take_screenshots.sh                      # manual Tauri debugging driver
└── run_quill_demo.sh                        # sandboxed backend/Tauri launcher

CLAUDE.md                                    # MODIFIED — SPECKIT block points at this plan
```

**Structure Decision**: `marketing-site/` remains the sole static deploy root. Canonical screenshots come from the real frontend through Browser Mock Mode and a standalone CDP driver. The sandboxed Tauri launcher and Python seeder remain separate backend-development tools.

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No constitution gate violations. Section intentionally empty.

## Phase 0 Research Summary

See [research.md](./research.md) for full Decision / Rationale / Alternatives entries. Topics resolved:

1. Static plain-HTML/CSS vs static-site-generator → **plain HTML**
2. Typography (FR-007 forbids Inter) → **Cabinet Grotesk-first display stack + local sans/mono fallbacks**
3. Rust env-var override pattern → **dedicated `data_paths.rs` module, opt-in via `QUILL_DEMO_MODE=1`**
4. Cross-platform launcher shape → **`.sh` + `.ps1` pair, no Python launcher**
5. Screenshot scope → **9 captures covering every marketed app surface**
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
- **Quickstart** (`quickstart.md`): build the capture image, drive Browser Mock Mode, inspect, preview, validate, and ship.

## Constitution Check (post-Phase-1 re-evaluation)

Re-evaluated after Phase 1 design.

- **Simplicity preserved**: Phase 1 introduced no extra runtimes, frameworks, or services. Contracts are short markdown documents; the data model is a directory tree; the quickstart is shell-and-keyboard maintenance instructions.
- **Scope preserved**: Every Phase-1 artifact maps to a clarified spec decision. No accidental scope expansion (e.g., no signup form contract, no analytics contract, no localization).
- **Reversibility preserved**: All Phase-1 deliverables are documents. None of them prescribe irreversible runtime behavior.

**Verdict**: PASS.

## Stop Conditions

Plan command ends after Phase 2 planning is implicitly defined (the structure tasks would take in `tasks.md`). The actual `tasks.md` is generated by `/speckit-tasks`, NOT by this command. No further work happens here.
