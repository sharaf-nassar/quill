# Infrastructure

Build tooling, CI/CD pipeline, release automation, and code quality enforcement for the Quill desktop application.

## Build Configuration

The frontend uses Vite with the React plugin; the backend uses Cargo with Tauri.

### Frontend Build

Vite serves on port 8181 in dev mode and ignores `src-tauri/**` to avoid extra frontend reloads during Rust rebuilds.

Production builds use esbuild minification and generate sourcemaps only when an
authenticated Sentry upload is configured. Other builds omit maps so native
packages cannot expose them. The build then rejects any remaining map before
Tauri can package it. The uncompressed chunk warning limit is 550 kB.
TypeScript uses strict mode, ESNext modules, and bundler resolution. See
`vite.config.ts` and `tsconfig.json`.

#### Crash Transport CSP

Production permits outbound frontend connections only to Tauri IPC and the exact HTTPS origin derived from the crash reporter DSN.

`index.html` names `https://o1373069.ingest.us.sentry.io` in `connect-src` so the browser SDK can post envelopes without widening the policy to every Sentry tenant. The dev-only policy in `vite.config.ts` keeps the same origin alongside its localhost tooling exceptions. `scripts/csp.test.mjs` pins the production allowlist to both IPC endpoints and the DSN origin.

### Backend Build

Rust edition 2024 uses the pinned `rust-toolchain.toml` compiler version. Crate types: `lib`, `cdylib`, `staticlib`. `build.rs` calls `tauri_build::build()`.

The `quill` app binary is Cargo's default run target; maintenance spikes under
`src-tauri/src/bin/` remain explicit `cargo run --bin <name>` targets.

The bundled SQLite driver (`rusqlite` with `bundled` feature) avoids system dependency issues. Tauri bundles Claude and Codex integration assets as app resources.

### Tauri Configuration

`src-tauri/tauri.conf.json` defines product name "Quill", identifier `com.quilltoolkit.app`, with a borderless transparent main window (280x340px, min 240x200).

Bundle targets: macOS app bundle + DMG, Windows NSIS, Linux AppImage. The Linux `.deb` was dropped because Tauri's updater only self-updates AppImages, so deb installs were stranded on their installed version. The `bundle.linux.deb.desktopTemplate` (`desktop-template.desktop`) is deliberately retained even with no `.deb` shipped: the AppImage bundler builds its AppDir via the shared Debian data generator (`appimage`'s `linuxdeploy` calls `debian::generate_data`), so that template still drives the AppImage `.desktop` entry — do not remove it as "unused deb config." Auto-updater uses GitHub releases endpoint with minisign public key verification, and macOS update detection depends on shipping the signed `.app.tar.gz` updater bundle in addition to the DMG installer.

## CI/CD Pipeline

GitHub Actions workflow (`.github/workflows/release.yml`) triggers on `v*` tags or manual dispatch.

Manual dispatch must select an existing `v*` tag. The `create-release` job rejects branch refs before checkout or draft creation, so version injection and packaging remain tag-only.

### Backend CI Gate

`.github/workflows/ci.yml` is the cross-platform Rust backend gate that also blocks release on failure.

It triggers on `pull_request`, `push` to `main`, and `workflow_call`, runs in `src-tauri` with `permissions: contents: read`, and pins Rust 1.95.0 plus the Cargo cache. The Linux job installs Tauri development packages and enforces `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`; the `macos-latest` job keeps `cargo check --all-targets` for AppKit-only code and runs the focused `runtime_backfill_` tests against bundled SQLite in the macOS filesystem/runtime environment before merge.

Because the base Tauri config enables `app.macOSPrivateApi`, the `tauri` dependency must keep the matching `macos-private-api` Cargo feature on every target; Tauri rejects config/feature drift before release.

`release.yml` calls it as a reusable workflow (`ci` job using `./.github/workflows/ci.yml`) and makes `create-release` `needs: ci`, so either a Linux gate failure or macOS compile failure blocks the entire build/sign/notarize/publish chain without duplicating release signing. Contract: `specs/005-learning-system-hardening/contracts/evaluation-harness.md`.

### Draft Release Pre-Creation

A `create-release` job validates the `v*` tag ref, then runs before all builds to create a single draft release. This prevents branch dispatches from creating releases and prevents parallel `tauri-action` instances from racing into separate drafts.

### Build Matrix

Four parallel builds (fail-fast disabled), all depending on `create-release` so `tauri-action` finds the existing draft.

`tauri-action` runs with `retryAttempts: 3` because its per-build `latest.json` uploads race on the shared release asset (tauri-action#1270); the publish job rebuilds that manifest deterministically regardless (see Release Publishing below).

Platforms: Linux (Ubuntu 22.04, AppImage), macOS Intel (x86_64), macOS ARM (aarch64), Windows (NSIS, runner pinned to `windows-2025`). Each installs Node.js 24 without an LTS-alias manifest lookup, trusts the runner system CA store for Node-based release clients, and installs the pinned Rust toolchain plus platform-specific system dependencies.

Unix free-space probes normalize `statvfs` counters to `u64` before multiplication because Apple exposes the fields with mixed integer widths; this keeps both macOS release targets compilable without changing overflow checks.

### Version Injection

Parses version from git tag (e.g., `v0.2.1` -> `0.2.1`) and updates `src-tauri/Cargo.toml` via sed before build.

Source builds retain the `0.0.0-injected-by-ci` sentinel. [[src-tauri/src/lib.rs#packaged_version_allows_updates]] disables their updater so newer local schema migrations cannot be replaced by an older published release; tag-injected builds keep normal version ordering.

`package.json` remains a frontend sentinel: Tauri package metadata comes from Cargo because `tauri.conf.json` omits `version`, while frontend crash reporting reads the tag from `VITE_APP_VERSION`. The release gate compares that tag and both Sentry identifiers directly with Cargo's injected version.

The Rust Sentry SDK prefixes Cargo's injected package version with `v`, matching `SENTRY_RELEASE`, `VITE_APP_VERSION`, and the GitHub tag on every release platform.

### macOS Code Signing

Imports APPLE_CERTIFICATE from secrets into a temporary build keychain and extracts CERT_ID for codesigning.

After build, submits DMG to Apple notary service (35-minute timeout), staples the notarization ticket, and re-uploads the notarized DMG to the release.

### Release Publishing

A third job (`publish`) waits for all builds, finds the draft release, and renames assets with platform labels (e.g., `Quill_0.3.1_macOS_amd64.dmg`).

It retries the draft lookup for API eventual consistency, then rebuilds `latest.json` from scratch and publishes the release. Because `tauri-action`'s parallel per-build `latest.json` uploads race on the single shared asset and silently drop platforms (this shipped v0.3.33 with no `linux-x86_64` entry, breaking the updater for Linux), the publish job is the manifest's single writer: after renaming assets it runs `.github/scripts/assemble-latest-json.sh`, which reads each platform's signed `*.sig` asset (distinct names never race) and emits the four base updater keys (`linux-x86_64`, `darwin-aarch64`, `darwin-x86_64`, `windows-x86_64`). The script fails the release if any base platform is missing, turning a silently broken manifest into a hard failure. The macOS build still verifies that `*.app.tar.gz` plus its `.sig` exist before continuing so the `darwin-*` signatures are present to assemble.

Asset URLs are constructed as `https://github.com/<repo>/releases/download/<tag>/<name>` rather than read from the draft's `browser_download_url`: the API reports draft assets under an ephemeral `untagged-<hash>` path that GitHub invalidates at publish time, which shipped v0.3.34 with dead updater URLs (Install silently no-oped; the manifest was hot-patched in place with corrected URLs).

### Required Secrets

`GITHUB_TOKEN`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`, `SENTRY_AUTH_TOKEN`.

### Sentry Source Map Upload

The Vite build uploads frontend source maps to the [[features#Crash Reporting]] Sentry project when `SENTRY_AUTH_TOKEN` is exported on the build step.

`vite.config.ts` reads `SENTRY_AUTH_TOKEN`, `SENTRY_ORG`, `SENTRY_PROJECT`, and `SENTRY_RELEASE` from `process.env` and instantiates `sentryVitePlugin` only when the token is set AND `NODE_ENV === "production"` (which Vite sets for `vite build`); dev runs and unconfigured CI jobs skip upload and map generation. Every release matrix leg receives the token because each runner builds its own frontend and cross-platform bundle equality is not assumed. The plugin injects deterministic debug IDs, uploads that leg's exact JavaScript and maps, then requests deletion of `dist/**/*.map`. Upload and injection failures propagate through its throwing error handler. `npm run build` finally runs `scripts/verify-sentry-output.mjs`, which rejects any remaining map (including a deletion error the plugin logs but swallows) or authenticated script without a debug ID before Tauri packaging starts. A second matrix-wide workflow assertion checks the final output.

All matrix plugins set `release.create`, `release.finalize`, and `release.setCommits` to false, so concurrent artifact uploads cannot race Sentry release management. The runtime event's release value can create the release on first use. The workflow's pre-build gate rejects missing auth or version drift; `SENTRY_RELEASE`, frontend `VITE_APP_VERSION`, and Rust's v-prefixed Cargo version all equal `github.ref_name`. [[crash-reporting-tests#Release matrix symbolication contract]] pins this matrix, plugin, and no-map contract without live Sentry credentials.

### Pages Workflow

`.github/workflows/pages.yml` deploys the marketing site to GitHub Pages using the official `actions/deploy-pages@v4` flow.

Triggers on `push` to `main` with a paths filter on `marketing-site/**` and the workflow file itself, plus `workflow_dispatch` for manual redeploys (useful when only screenshots change). Two-job split: `build` checks out the repo, runs `actions/configure-pages@v5`, and uploads `marketing-site/` verbatim via `actions/upload-pages-artifact@v3` (no build step — the site is plain static HTML/CSS/JS); `deploy` consumes the artifact and runs `actions/deploy-pages@v4` against the `github-pages` environment so the deployed URL surfaces in the Actions UI. Permissions follow the GitHub-recommended least-privilege template: `contents:read`, `pages:write`, `id-token:write`. The `pages` concurrency group is set to `cancel-in-progress: false` to match GitHub's recommendation that an in-flight Pages deploy not be killed mid-flight. Contract: `specs/001-marketing-site/contracts/pages-workflow.md`.

### Marketing Site

The marketing site is a static GitHub Pages deliverable that sells Quill through real product screenshots and stable anchored sections.

`marketing-site/index.html` owns the single-page content and the public `#hero`, `#analytics`, `#context`, `#search`, `#live`, `#learning`, `#memory`, `#brevity`, and `#install` fragments (the original seven are a stable deep-link contract; `#memory` and `#brevity` were added 2026-06-19). `marketing-site/styles.css` owns the Signal Theater visual system: Quill's quiet dark app surface, actual logo mark, cyan/purple logo accents, clipped geometry, dense screenshot proof, and an alternating two-column spotlight rhythm that shows each lean per-section screenshot whole at its natural aspect (no cover-cropping), with self-hosted Space Grotesk (display) and Geist (body) woff2 fonts under `assets/fonts/` (OFL, preloaded, no remote fonts). `marketing-site/motion.js` progressively adds the `.motion-rise` reveal with native `IntersectionObserver` plus CSS transitions. Reduced-motion clients and browsers without the observer skip the pending class, so content stays readable without JavaScript or motion support.

The stylesheet link includes a version query so palette changes are not masked by stale browser caches during local preview or GitHub Pages deploys.

The hero is product-led and two-column on desktop: the left column holds a short problem-first headline ("Stop running your coding agents blind."), a deck line on its own row between the headline and the description carrying the bidirectional hook (you get the insight; your agents get the tools), a one-line description, install/source actions, and a trust line (own-plan, no API key, no tracking, MIT) — all left-aligned and vertically centred beside, in the right column, one slim product window (the 360px widget on its Usage view — the LIMITS band above the six-hour chart, readout grid, and session breakdown) under a cyan/purple glow at a 360px stage cap that matches the widget's own width. The shot is shown whole: the widget frame ends at its footer row, so the height clip and bottom fade the taller split-pane capture needed were removed with it. Placing the window beside the copy rather than stacked under it keeps the tall shot space-efficient; under 980px the hero collapses to a single centred column (copy then window). Standalone KPI strips are avoided because the screenshot carries the evidence more credibly.
The marketing copy features Claude Code and Codex only; MiniMax was dropped from the site on 2026-06-23 (the desktop app still tracks MiniMax live limits — see [[features#Live Usage View]]). The product window peeks below the fold to invite the scroll into the lead Analytics spotlight.

The page is ordered for the narrative "analytics first, then the agent tools built on it" as a single alternating two-column spotlight rhythm — Analytics, Context/MCP (foregrounding that the agent itself calls Quill's `quill_*` tools), Search, Live, Learning, then the Memory and Brevity agent tools — each section pairing its copy with one full product screenshot on the opposite side, alternating which side the image sits on down the page, and closing on install/trust. Every screenshot is a single section captured on its own, cropped lean to its content (no window-chrome excess, no scrollbars, no dead space) and shown whole at its natural aspect rather than cover-cropped or combined with other panes. Each shot is also displayed at — or below — its native retina width via a per-section `--shot-w` custom property (360px for the four widget shots, matching the widget's own width; 520px for sessions/learning/memory, 560px for brevity; 480px default) feeding the media grid track `minmax(0, var(--shot-w))`, so the slim product window is never upscaled wider or taller than captured and the copy column takes the remaining space. `#live` is the one section that does not show its shot whole: the widget is a single frame, so LIMITS has no capture of its own and the `.shot-band` frame clips `live.png` to that band's hairline (`aspect-ratio: 360 / 185`) rather than repeating the hero shot two sections later. Feature copy uses short, concrete, screenshot-backed claims instead of long paragraphs, keeping the page scannable while preserving the technical details developers need.

The visual contract is documented in `marketing-site/README.md` and `specs/001-marketing-site/spec.md`; screenshot assets still come from the sandboxed demo workflow described under [[infrastructure#Scripts#Screenshot Capture]].

## Release Process

`release.sh` automates version bumping, release note generation, and tagging.

### Commands

Available subcommands for the release script.

- `./release.sh [--ai auto|codex|claude] bump <major|minor|patch>` — Bump version, auto-select a release-notes CLI, create annotated git tag, and push to trigger CI
- `./release.sh [--ai auto|codex|claude] retag [version]` — Re-point existing tag to HEAD and optionally regenerate notes with the selected CLI
- `./release.sh latest` — Show current version

### AI Release Notes

Uses `codex` when installed, otherwise falls back to `claude`; `--ai claude` or `--ai codex` overrides the default selection.

The Codex path pins `gpt-5.5`, `model_reasoning_effort="xhigh"`, and `service_tier="fast"` in ephemeral non-interactive mode, forces `-C` to the git repo root, and leaves Claude on its existing inference path.

Codex writes its native execution stream directly to the terminal while `--output-last-message` captures only the final response in a temporary file for the release body. `release.sh` adds no spinner, filtering, panel renderer, polling loop, or duplicate log buffer.

Prompt instructs the model to focus on user-visible features only, omitting refactors, dependency updates, and CI changes. It opens with a short value summary, then uses only the non-empty Highlights, Improvements, Fixes, and Upgrade Notes sections.

## Code Quality

Linting, formatting, and pre-commit enforcement for both frontend and backend code.

### ESLint

Flat config format (v9+) in `eslint.config.js`. Base: `@eslint/js` recommended + `typescript-eslint` strict. Unused vars starting with `_` are allowed. Scope: `src/**/*.{ts,tsx}`. Max warnings: 0 (enforced by pre-commit).

### Pre-Commit Hooks

`.pre-commit-config.yaml` runs on every commit:

| Hook | Scope | Purpose |
|------|-------|---------|
| trailing-whitespace | All | Strip trailing spaces |
| end-of-file-fixer | All | Ensure newline at EOF |
| check-yaml, check-json | All | Syntax validation |
| check-merge-conflict | All | Detect unresolved conflicts |
| check-added-large-files | All | Flag files >500 KB |
| detect-private-key | All | Catch hardcoded secrets |
| shellcheck | `*.sh` | Shell script linting |
| cargo fmt | `src-tauri/**` | Rust formatting |
| clippy | `src-tauri/**` | Rust linting across all host targets (`-D warnings`) |
| eslint | `src/**/*.{ts,tsx}` | TypeScript linting |
| tsc --noEmit | `src/**/*.{ts,tsx}` | Type checking |

The `clippy` hook runs `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` directly. Platform-gated code is linted by committers and CI on its matching host.

### Dead-Code Gate

`npm run knip` must report nothing — zero unused files, exports, types, and dependencies across `src/**/*.{ts,tsx}`.

Config lives in `knip.json`: project scope `src/**/*.{ts,tsx}`, `ignoreExportsUsedInFile` on (a symbol consumed only inside its own module is not "unused"), checks limited to files, dependencies, exports, and types. Enum members and duplicate exports are excluded because neither is a reliable deletion signal here.

The gate is absolute, not differential. The widget redesign temporarily carried a `knip-baseline.txt` recording pre-existing debt so the legacy teardown could prove it added no new unused surface; that debt is now gone and the baseline file with it. There is no allowlist to append to — a new report line means the code is dead and must be deleted or wired up.

Two categories of finding recur. A component that exports both `export function X` and `export default X` while every consumer imports the default: drop the named export, keeping the repo's `function X(...)` + `export default X` shape. An npm package whose Rust counterpart is what the app actually uses (`@tauri-apps/plugin-process` and `@tauri-apps/plugin-window-state` were both removed for this reason): drop the JS binding, keep the `src-tauri/Cargo.toml` crate.

## Scripts

Utility scripts for development, testing, and documentation tasks.

`npm test` runs every `scripts/*.test.mjs` file through Node's native, quoted glob handling. Focused verification can invoke a test file directly with `node --test`.

### Screenshot Capture

`scripts/take_screenshots.sh` drives a running Quill instance with xdotool and ImageMagick, capturing every widget view and Manage section unattended into the canonical per-section PNG files.

Navigation is keyboard-first because the widget kept no fixed button strip to click. The view dropdown's y-position moves with the LIMITS band (one row per enabled provider), so the script Tab-walks focus while pressing ArrowDown after each hop and recognises the opened listbox by counting pixels painted in the raised menu colour `#1b212b` — unique to [[src/components/widget/ViewSwitcher.tsx#ViewSwitcher]]'s popup, and measured across the 360px renders in `specs/018-widget-ui-redesign/verification/` at 373–3617 for every closed state against 17917 for the open one. It then picks a row absolutely with Home + ArrowDown ×N + Enter, so the target never depends on which view was showing; the row order mirrors `VIEWS` in [[src/components/widget/ViewRegion.tsx#ViewRegion]]. The Manage workspace opens with Ctrl+M and each section is selected by typing its label into the Ctrl+K command palette, so no rail coordinate is hardcoded either. The one surviving offset is a click into the LIMITS padding under the fixed 40px titlebar, which drops keyboard focus so the global `:focus-visible` ring never reaches a published shot.

Output is `hero.png` (widget on the Usage view), copied to `live.png` because the LIMITS band the `#live` section sells shares that same 360px frame, plus `models.png`, `analytics-context.png` (Context view, anchor `#context`), and one shot per Manage section (`sessions.png`, `learning.png`, `instances.png`, `settings.png`). The `#analytics` section and social metadata reuse `hero.png`. `memory.png` and `brevity.png` stay manual because each is a sub-panel rather than a section. Every grab is tested for zero standard deviation and the run exits non-zero when any shot came back blank, so the GL-surface failure below is reported rather than silently published.

Default output directory is `marketing-site/assets/screenshots/` (overridable via `OUTDIR=...`). Captures use ImageMagick `import` then upscale 2× via `convert -filter Catrom -resize 200%` for HiDPI rendering on the marketing site (override with `RETINA=0`).

What is published today came from the 2026-08-01 refresh, the first taken against a live build rather than a render: every widget shot is a real `import` grab of a sandboxed instance, so no view switcher carries a `:focus-visible` ring. `hero.png` and `live.png` are the widget on its Usage view (LIMITS band, six-hour two-provider chart, full readout grid, session breakdown), and `analytics-context.png` is the Context view. `sessions.png` and `learning.png` are Manage sections at the workspace's own 960×680, replacing the 2026-06-24 shots of the standalone windows that no longer exist; `memory.png` and `brevity.png` are cropped out of the Learning and Settings sections that now host them. Social metadata uses the Usage screenshot directly. The root `README.md` embeds these canonical `marketing-site/assets/screenshots/` files; no root screenshot mirrors are maintained.

That refresh also showed the documented GL-surface constraint no longer holds on the maintainer's GNOME/Mutter (X11) host: `import` reads the `target/debug` binary's window fine, and a plain `cargo build` embeds `frontendDist` rather than loading `devUrl`, so the AppImage detour is unnecessary. The demo instance instead runs under `dbus-run-session` with an isolated `HOME` — both now owned by [[infrastructure#Scripts#Demo Launcher]] rather than by hand — which gives it its own `tauri-plugin-single-instance` lock so it can coexist with the maintainer's own Quill, and keeps the demo's provider-enable, brevity and memory writes off the real `~/.claude`. Any second window titled `Quill` must be unmapped for the run, because the script resolves its target by window name.

The seeded dataset is photographed as-seeded; the grooming each shot used to need is now the seeder's job — see [[infrastructure#Scripts#Dummy Data Seeder]]. `populate_dummy_data.py` asks the selected Quill executable to create or migrate `usage.db` before it writes fixtures, so every table and `schema_version` row comes from the Rust production migration path. Each `code_change` row stores its own `lines_added`/`lines_removed`, which is what TOK/LOC, LOC/HR, and NET LINES read. Tokens, tool actions and turns are attributed across `claude` and `codex`, so the usage chart draws two series, the breakdown rows carry real turn counts and two of them roll up a sub-agent. Claude reports three limit buckets — the plan pair plus one model window — which is the composition the 360px LIMITS row and the mockup share, with exactly one cell in the amber band. MiniMax is seeded enabled but not installed, the state that renders the SETUP row the `#live` copy describes. Confirmed rules are written into `<rules>/{claude,codex,shared}` where demo mode scans for them, so ACTIVE RULES is populated and scope badges vary, while candidates get no file and an empty `file_path` — exactly how the app stores a discovered rule.

The compositions each shot has to reach survived the window rewrite even though the windows did not: `sessions.png` wants a query typed and a result detail open, `learning.png` wants active rules above discovered candidates with no empty section, `memory.png` wants a clean "All Projects (4)" with one file per project, and `brevity.png` wants the toggle on with its explanation visible. Reaching them needs three manual steps the script does not take — typing a query into Session Search, opening the Learning section's Memories tab, and opening Settings → Context. The fourth, removing the global `~/.claude/CLAUDE.md` that `claude_setup` recreates on every launch so the Memories count stays at four projects, is now done by the launcher's watcher. `analytics-context.png` needs context preservation enabled with `context_savings_events` rows, which the seeder populates. The seeder's `PROJECTS` deliberately use single-segment paths (e.g. `/home/alex/gateway`, never `/home/alex/api-gateway`) because the panel's slug↔path round-trip ([[src-tauri/src/memory_optimizer.rs#project_path_to_slug]] encodes `/`→`-`, [[src-tauri/src/memory_optimizer.rs#slug_to_path]] decodes `-`→`/`) cannot tell an internal dash from a path separator; single-segment names keep the round-trip unambiguous so the panel shows clean, de-duplicated project headings. Capture filenames map 1:1 to the marketing site's anchored sections per `specs/001-marketing-site/data-model.md`; the page then displays each at or below its native retina width (see [[infrastructure#CI/CD Pipeline#Marketing Site]]).

The memory files the Memories panel lists need the isolated `HOME`, not the session roots: [[src-tauri/src/memory_optimizer.rs#memory_dir]] resolves them under `dirs::home_dir()` rather than through `QUILL_CLAUDE_PROJECTS_DIR`. The launcher therefore hands its sandbox home to the seeder as `--home-dir`, and [[scripts/populate_dummy_data.py#populate_memory_markdown]] writes `$HOME/.claude/projects/<slug>/memory/<name>.md` with `type:` and `description:` frontmatter — one per seeded project, which is what produces the CONTEXT / CONVENTION badges and the one-line descriptions. The flag is opt-in because a default run would otherwise plant fictional projects in the maintainer's real home. The panel's "All Projects (N)" counts every per-project context file, which includes the global `~/.claude/CLAUDE.md` that `claude_setup` recreates on each launch (counted once per project — see [[src-tauri/src/memory_optimizer.rs#get_known_projects]]); a clean "All Projects (4)" therefore needs that file gone after launch but before the panel is first opened, since the panel counts on window mount and `claude_setup` does not run again until the next launch — which is precisely the window the launcher's bounded watcher covers.

### Dummy Data Seeder

`scripts/populate_dummy_data.py` seeds deterministic sample data into a database initialized by Quill's current Rust migrations.

By default, checks that Quill is not running before modifying the personal DB, creates a backup, and writes sample rules. Sandbox mode accepts isolated data, rules, Claude, and Codex paths. Legacy Claude-only reruns replace regular generated targets but still refuse symlink/junction paths. Complete mode rejects production overlap, marks owned JSONLs, removes only marked prior files, refuses symlink/junction descendants and existing targets, and uses exclusive writes. It declares completion only when runtime discovery equals seeded canonical sources; after a post-core failure it attempts best-effort pending/incomplete recovery, warns if recovery fails, and preserves the original error. Windows source keys restore Rust's verbatim canonical path before UTF-16BE hex encoding. Fixtures include dynamic/unattributed chains and 1,001 generated IDs without a catalog. Full CLI surface in `specs/001-marketing-site/contracts/seeder-cli.md`.

Schema fidelity has one owner. [[src-tauri/src/lib.rs#initialize_database]] opens the requested `usage.db` through `Storage` and applies every production schema migration without starting Tauri or unrelated startup cleanup. The seeder never creates tables or writes `schema_version`; it only clears and inserts fixture data after initialization succeeds.

Reruns preserve migration authority. Legitimate older databases migrate before seeding, newer or inconsistent schemas fail instead of being destructively rebuilt, and the launcher's `--clean` remains the sandbox recovery path. Seeder-owned hourly rows and runtime state are cleared, while `rollup_meta` resets deterministically to pending with null bookmarks so the app rebuilds rollups from the new fixture evidence.

Complete model-fixture mode writes retained sources first: [[scripts/populate_dummy_data.py#populate_session_jsonls]] returns descriptors for Claude parent/subagent JSONLs, and [[scripts/populate_dummy_data.py#populate_codex_session_jsonls]] does the same for Codex root/child rollouts. [[scripts/populate_dummy_data.py#populate_model_analytics]] then re-discovers both configured roots, requires exact equality with those descriptors, hashes and stats the written bytes, derives each DB key through [[scripts/populate_dummy_data.py#canonical_model_source_key]], and persists source rows plus observations from the same record objects. It marks migration-28 backfill complete only after both inventories match, keeping seeded analytics replay-coherent with runtime JSONL discovery.

### Demo Launcher

`scripts/run_quill_demo.sh` (POSIX) launches a sandboxed Quill instance against dummy data without touching the maintainer's personal Quill state; the former PowerShell launcher was removed.

The launcher creates a stable per-user sandbox directory (`/tmp/quill-demo-$USER`), passes the selected Quill executable plus the same Claude and Codex directories to the seeder, then exports the source roots with `QUILL_DEMO_MODE=1`. Runtime discovery therefore reads the exact roots whose canonical sources populated SQLite; neither production session root is read. It also isolates data and rules, runs with `--no-backup --quiet`, then executes that same Quill build. It additionally isolates `HOME` to `$SANDBOX/home` and passes it as `--home-dir`, because the app resolves memory documents and its own context assets from the home directory rather than from the `QUILL_*` overrides; it launches under `dbus-run-session` when available so the demo owns a private session bus and therefore its own single-instance lock, and runs a bounded watcher that deletes `<home>/.claude/CLAUDE.md` once the app recreates it. See [[backend#Data Paths#Demo-mode path override]] and `specs/001-marketing-site/contracts/launcher-cli.md`.

### macOS Bootstrap

`scripts/mac.sh` bootstraps a macOS 14+ machine by installing Homebrew with the current official installer, then refreshing Homebrew metadata before installing or upgrading the moving `node` formula and `docker-desktop` cask.

The script exits early on non-macOS hosts and unsupported macOS releases so failures are explicit. It treats Docker as Docker Desktop on macOS because that installs the app/runtime rather than only the standalone `docker` client binary.

## MCP Verification Environment

MCP import verification for Claude and Codex removes inherited `PYTHONHOME`
and `PYTHONPATH`. This isolates `uv run ... python` from packaged-launcher
variables while preserving each provider's normal runtime environment.

## Claude Integration Deployment

Claude integration lives directly in [[src-tauri/src/claude_setup.rs]] —
detection plus manifest-aware install/uninstall — while startup orchestration
remains in [[src-tauri/src/integrations/manager.rs]]. The former
`integrations/claude.rs` adapter shim was deleted; callers use `claude_setup`
directly.

[[src-tauri/src/claude_setup.rs#ClaudePaths]] resolves a non-empty `CLAUDE_CONFIG_DIR` as Claude's user directory. Without the variable it uses `~/.claude` and legacy `~/.claude.json`; with it, settings, commands, instructions, hooks, and `.claude.json` all live inside that directory. A versioned state file pins the selected paths so repair and uninstall do not follow a later environment change. Before state exists, a detected legacy Quill install at the default paths wins over a new override so it can be repaired or removed safely.

[[src-tauri/src/claude_setup.rs#preflight_configuration]] parses `settings.json`, its nested hook shape, `.claude.json`, and `mcpServers` before deployment or uninstall mutates anything. Malformed or structurally incompatible JSON returns an error without backup-and-replace behavior. The transaction then snapshots both provider files plus Quill's ownership state.

First install records whether `mcpServers.quill` existed and, if so, its exact prior JSON value. Quill replaces only that key while enabled; [[src-tauri/src/claude_setup.rs#restore_quill_mcp]] restores the captured value on uninstall or removes the key when it was absent, preserving every unrelated MCP server and root metadata field.

### Deployed Assets

Files and directories created during first-launch auto-deployment.

| Target | Content |
|--------|---------|
| `~/.config/quill/scripts/` | Base CJS hook scripts for token reporting, session sync, and qbuild edit guarding. `observe.cjs` is added when activity tracking is enabled (default on). Context routing is added when context preservation is enabled, plus `context-telemetry.cjs` when context telemetry is also on (default on, gated on context preservation) |
| `~/.config/quill/mcp/` | Python MCP server for session querying; working-context tools only when context preservation is enabled |
| `<Claude config>/commands/` | Custom CLI commands; `<Claude config>` is `CLAUDE_CONFIG_DIR` or `~/.claude` |
| `<Claude config>/settings.json` | Hook registrations marked with advisory `_source: "quill-setup"` or `quill-context-preservation` metadata |
| `<Claude config>/.claude.json` | MCP server registration when `CLAUDE_CONFIG_DIR` is set; the default installation keeps Claude's legacy `~/.claude.json` location |
| `<Claude config>/CLAUDE.md` | Quill MCP usage instructions injected as one exact managed block between `<!-- quill-managed:claude:start -->` / `<!-- quill-managed:claude:end -->` markers. The base template drops the working-context pointer when context preservation is off |
| `~/.config/quill/claude/integration-state.json` | Mode-`0600` path and ownership state: main/restart component flags plus the captured prior `mcpServers.quill` value |

Managed CJS files are data files, not executables. Hook entries use Claude's exec form with an absolute Node executable in `command` and the absolute script path in `args`; qbuild receives the resolved Git executable as a second arg. Setup requires Node.js 18+ and Git before mutation. The ownership state is mode `0600` on Unix.

Deployment stamps hash the bundled managed assets and enabled features. Any changed managed asset makes an installation stale; the next [[src-tauri/src/integrations/manager.rs#repair_provider]] reinstalls it and refreshes the stamp without a version bump.

[[src-tauri/src/claude_setup.rs#remove_matching_hook_handlers]] removes only exact Quill command handlers from each matcher group's nested `hooks` array. Third-party siblings, matchers, and unknown group metadata survive; only groups and event keys emptied by owned removal are pruned. Ownership uses exact exec-form command/args identities plus explicit retired shell-form paths, never a serialized-group substring. `_source` remains advisory because Claude may discard unknown fields.

[[src-tauri/src/claude_setup.rs#verify_hook_settings]] requires one exact copy of every expected group and the exact total number of managed handlers, including command, args, matcher, and timeout. Verification also requires the expected MCP object, one current instruction block, complete ownership state, and feature-consistent asset presence before commit.

### Hook Event Matrix

Claude hook coverage is explicit and limited to events that drive Quill behavior.

| Gate | Event / matcher | Handlers and timeout |
|------|-----------------|----------------------|
| Always | `PreToolUse` / `Edit\|Write\|NotebookEdit` | `qbuild-guard.cjs` (5s) |
| Always | `Stop` / all | `report-tokens.cjs` (5s), `session-sync.cjs` (10s) |
| Always | `StopFailure` / all | `session-sync.cjs` (10s) |
| Always | `SessionEnd` / all | `session-sync.cjs` (10s) |
| Activity tracking | `PreToolUse`, `PostToolUse`, `PostToolUseFailure` / `*` | `observe.cjs` (3s) |
| Activity tracking | `Stop`, `StopFailure`, `SessionEnd` / all | `observe.cjs` (3s) |
| Context preservation | `PreToolUse` / `*` | `context-router.cjs` (5s) |

`observe.cjs` maps `PostToolUseFailure.error` into post-phase output while keeping the high-signal post gate (`Bash`, `Edit`, `Write`, `NotebookEdit`). `src-tauri/claude-integration/scripts/qbuild-guard.cjs` accepts both `file_path` and `notebook_path`, resolves the nearest existing ancestor for new targets, and checks lexical plus canonical containment so `..` and symlink paths cannot escape the main-checkout guard.

`session-sync.cjs` runs only on `Stop`, `StopFailure`, and `SessionEnd`. Its `SyncBudget` caps one hook run at 8 seconds and 18 HTTP attempts: enough to isolate a poisoned first row in a 500-row chunk, acknowledge its valid siblings, and begin another chunk. Remote sync sends successive ≤500-message chunks without splitting a normal user/assistant pair. It checkpoints only contiguous accepted or deliberately dropped segments; retryable and envelope-level failures leave the cursor at its prior acknowledged prefix.

Startup repair in [[src-tauri/src/integrations/manager.rs#repair_provider]] reads the complete `IntegrationFeatures` set and reinstalls every enabled provider with that matrix.

After provider repair, startup runs the one-way retired-session-context cleanup. It first removes old capture handlers from Claude and Codex config, using Quill script-directory ownership so foreign siblings and metadata survive; the Codex no-CLI fallback remaps opaque `hooks.state` entries by source and position. Only after both provider configs are safe does [[src-tauri/src/integrations/retirement.rs#purge_continuity_artifacts_at]] remove the two installed capture scripts, the retired context subtree, two retired tables from an existing context database, and five retired analytics event types. The pass is idempotent, never creates a missing database, and never vacuums unrelated working-context data.

## Codex Integration Deployment

Codex integration lives in [[src-tauri/src/integrations/codex.rs]] and deploys provider-specific assets under `~/.config/quill/codex/` plus Quill-managed entries in the user's Codex home.

### Deployed Assets

Files and config entries created when the Codex provider is enabled.

Deployment is allowlisted to token and sync scripts by default, plus the shared `lib.cjs` helper (config load, local-only guard, and timeout-bounded HTTP POST) they and `hook-observe.cjs` require. `observe.cjs` and `hook-observe.cjs` are added when activity tracking is enabled. Context routing is deployed only when context preservation is enabled, with `context-telemetry.cjs` further gated on the context telemetry flag. The Claude-only `qbuild-guard.sh` is never copied into Codex assets.

| Target | Content |
|--------|---------|
| `~/.config/quill/codex/scripts/` | Base hook scripts for token reporting and session sync. `observe.cjs` is added when activity tracking is enabled (default on); `hook-observe.cjs` rides with the same flag and ships hook-fire telemetry for the Now-tab Hooks breakdown via `POST /api/v1/hooks/observed`. Context routing is added when context preservation is enabled, plus `context-telemetry.cjs` when context telemetry is also on |
| `~/.config/quill/codex/mcp/` | Python MCP server copied from the bundled Quill MCP assets; working-context tools only when context preservation is enabled |
| `~/.config/quill/codex/templates/` | Managed AGENTS template block |
| `<Codex home>/config.toml` | `features.hooks = true`, inline `[[hooks.*]]` Quill hook registrations, reconciled Codex `hooks.state` trust hashes, and Quill MCP environment values. Codex home is non-empty `$CODEX_HOME` when set, otherwise `~/.codex` |
| `<Codex home>/AGENTS.md` | Managed Quill session-history guidance block. Kept slightly richer than Claude's by naming key working-context tools because Codex is not guaranteed to surface MCP server `instructions`; the `agents-md-section-base.md` variant (context preservation off) carries only the `search_history` route and the raw-JSONL prohibition |
| `~/.config/quill/codex/integration-state.json` | Versioned uninstall state: selected Codex home, prior `features.hooks` value, whether `mcp_servers.quill` pre-existed, and prior Quill MCP environment values |

Codex install and uninstall remove only Quill-owned `hooks.json` and inline-TOML commands; non-Quill hooks survive. Config mutation uses `toml_edit` table semantics, so standard, dotted, and inline-table spellings resolve to one logical key instead of producing duplicates. First install records the user's prior `features.hooks` and Quill MCP environment values. Uninstall restores those values, removes values that were previously absent, and removes the whole MCP entry only when Quill created it. Ownership of `mcp_servers.quill` is judged structurally by [[src-tauri/src/integrations/codex.rs#mcp_entry_is_quill_owned]]: an entry whose `command` or `args` point into `~/.config/quill/codex/mcp` is Quill's, matching how `command_uses_quill_script_directory` claims hooks. Marker comments cannot carry that judgement because Quill strips them from every config it writes, so an install running without `integration-state.json` — a lost file, or an install wedged before the state write — would otherwise adopt Quill's own entry as user-owned and leave it registered against a directory uninstall deletes. Provider scripts use bounded HTTP waits so a slow widget cannot hold Codex hooks open until the host kills them.

Codex installs SessionStart, UserPromptSubmit, and Stop hooks unconditionally; the unconditional SessionStart group uses no matcher so it covers every session start. PreToolUse and PostToolUse `observe.cjs` hooks ride with the activity tracking flag and register on the canonical `Bash|apply_patch` matcher; Codex reports unified `exec_command` as `Bash`, so no separate matcher is needed. When context preservation is enabled, Codex also installs the PreToolUse routing hook. When activity tracking is enabled, the installer additionally registers `hook-observe.cjs` without a matcher on eight observed Codex hook events (`PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `UserPromptSubmit`, `SessionEnd`, `Stop`). Stop supplies prompt turn-end timing and SessionEnd remains fallback; [[data-flow#Data Flow#Live Session Tracker|rollout transcripts]] remain authoritative for positive state. Other lifecycle events stay unregistered, while `CODEX_HOOK_EVENTS` still lists all eleven so repair and uninstall prune older registrations.

Codex has shipped hooks default-enabled since Codex 0.124.0, but Quill sets semantic `features.hooks = true` while its managed hooks are installed and restores the prior explicit value or absence on uninstall. The deployed `.cjs` bridges resolve a stable session id through a `session_id || conversation_id || id` fallback chain (`hook-observe.cjs` falls back further to `""`; `session-sync.cjs` still requires both an id and a `transcript_path` before it syncs). `hook-observe.cjs` forwards optional `agent_id` attribution when Codex supplies it and sends `hook_matcher: null` because Codex command-hook stdin does not expose the configured matcher.

The installer calls Codex `hooks/list` before and after hook mutation. It remaps opaque third-party `hooks.state` records by source, event, current hash, and duplicate order; removes stale Quill state; then writes each live Quill key with Codex's returned hash. Verification parses the resulting TOML and requires the exact expected Quill handlers to be enabled and trusted in a fresh `hooks/list` response.

Initial install honors a non-empty `$CODEX_HOME` and otherwise uses `~/.codex`; an empty value or a path occupied by a non-directory is rejected. The selected home is persisted in `integration-state.json`, so reinstall, verification, and uninstall keep targeting the original configuration even if the environment later changes. Legacy installs without state uninstall from `~/.codex`. Every spawned app-server receives the selected home through `CODEX_HOME`.

Quill resolves the Codex CLI before running provider checks or `codex app-server`, then augments the child process `PATH` with launcher and symlink-target directories so Homebrew and npm installs work from macOS app launches with stripped inherited environments.

### App-Server Request Contract

[[src-tauri/src/integrations/codex.rs#run_app_server_request]] is the single one-shot `codex app-server` path. Hook registration and Codex usage polling differ only in feature flag, client identity, `CODEX_HOME`, and deadline.

Each call spawns the CLI, sends `initialize`, `initialized`, and one request at id 2, then reads stdout until that id answers or the caller's deadline expires. Hook work uses ten seconds because it runs at startup holding the process-wide mutation guard; usage polling uses thirty because the child round-trips to the ChatGPT backend.

Teardown is the load-bearing part. The `codex` entry point is typically an npm wrapper that re-execs the platform binary as a grandchild, so signalling only the direct child orphans a process that still holds the stdout pipe open. [[src-tauri/src/integrations/codex.rs#ReapedChild#spawn]] therefore puts the child in its own process group and [[src-tauri/src/integrations/codex.rs#ReapedChild#terminate]] signals that group before reaping — signalling first, because `wait` frees the pid for reuse.

The reader thread is deliberately never joined. Group termination closes the pipe, so it ends on its own; the only case a join would cover is a child that escaped the group, and that is precisely the case where joining blocks the caller — and the mutation guard it holds — instead of returning the timeout already computed. A leaked reader is bounded; a wedged startup repair silently freezes every later provider mutation.

Process groups are a `#[cfg(unix)]` mechanism. Windows is not a supported target, and the unconditional deadline plus non-blocking teardown keep the failure there bounded to a leaked child rather than a wedge.

## Pi Integration Deployment

[[src-tauri/src/integrations/pi.rs]] owns Pi detection, extension deployment, managed instructions, transactional removal, and startup repair.

Pi must resolve through the shared login-shell, launcher, and symlink-aware PATH logic and report version 0.84.0 or newer. A resolved CLI with old or unparseable output, invalid persisted paths, or a non-writable extensions directory produces an error status with `last_error`; saved status merging preserves that fresh failure.

The selected `$PI_CODING_AGENT_DIR` and `$PI_CODING_AGENT_SESSION_DIR`, or their `~/.pi/agent` defaults, are captured in `~/.config/quill/pi/integration-state.json` with the detected Pi version. Later repair and removal use those persisted paths even if the environment changes.

Installation copies the bundled `quill.ts` to `<Pi config>/extensions/quill.ts` and maintains one Quill block in `<Pi config>/AGENTS.md`. The extension payload marker and managed block markers define ownership. A user-owned `quill.ts` blocks installation; unrelated extension files and user instruction bytes remain untouched. Disabling removes only marked Quill files, the managed block, state, and stamp. It does not delete indexed Pi sessions or analytics.

Pi uses [[src-tauri/src/integrations/deploy.rs#FileSnapshots]] as a configuration-only transaction over individual files. It never stages or renames the extensions directory, so sibling extensions cannot enter Quill's backup. The global mutation guard invokes Pi recovery before every provider mutation.

Startup repair takes the same fast path as Codex: a deployment is current only when its bundled-source stamp matches and semantic verification passes. Verification compares exact extension bytes, checks the payload marker, rejects extra Quill-marked extension files, requires the current AGENTS block, and parses the four-field integration state. Tauri bundles `pi-integration/**/*`, and [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Packaged Assets]] pins that package input.

### Extension Tools and Telemetry

The single-file Pi extension exposes Quill's local history and working-context APIs while failing closed on non-loopback configuration.

Install renders `context_preservation`, `activity_tracking`, and `context_telemetry` into the owned payload and deployment stamp. Context preservation registers eight plain-JSON-Schema `quill_` tools plus Pi's context router; activity tracking maps Pi lifecycle events to existing hook names with provider `pi`.

The router ports the canonical Claude/Codex fetch and tainted-read policy to Pi's `bash`, `read`, and fetch tool inputs. It returns Pi's `{ block, reason }` result, persists at most 256 tainted paths per session, and names ready `quill_` replacements in every denial. Turning context preservation off omits the router entirely after `/reload`.

Tool requests use the main local URL for session history and the separate context origin for `/api/v1/context/*`. Both require exact loopback hostnames and share Codex's 1500 ms local timeout. Telemetry starts bounded requests without awaiting them. Context telemetry remains dependent on context preservation and posts Pi routing events with all routing token estimates set to zero.

The file transaction also sets `context_http.enabled=true`; removal clears it because Pi is the only installed listener consumer. Recovery reconciles the setting with the restored owned extension. [[pi-extension-tests]] and [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Context HTTP Setting]] pin these boundaries.

## Provider CLI Detection

Claude, Codex, and Pi CLI detection runs through [[src-tauri/src/config.rs#resolve_command_path]] with an invalidatable login-shell PATH cache so the integrations menu's "Rescan PATH" action can pick up new installs without restarting Quill.

Detection layers a login-shell `command -v` lookup with a static fallback list and dynamic per-package-manager prefix queries. The cache lives in an `RwLock` and is cleared via [[src-tauri/src/config.rs#refresh_shell_path]] when the UI calls [[src-tauri/src/integrations/manager.rs#force_rescan]].

The static fallback list covers per-user package managers that frequently aren't in the login-shell PATH because users add them only to interactive shell config (`~/.zshrc`, `~/.bashrc`) which `zsh -lc` does not source: `~/.bun/bin`, `~/.cargo/bin`, `~/.deno/bin`, `~/.volta/bin`, `~/.local/bin`, `~/.local/share/pnpm`, `~/.npm-global/bin`, `~/n/bin`, `~/.yarn/bin`, `~/.config/yarn/global/node_modules/.bin`, `~/.nix-profile/bin`, `~/.asdf/shims`, `~/.nodenv/shims`, `~/.local/share/mise/shims`, plus Anthropic's `claude migrate-installer` target `~/.claude/local/{,node_modules/.bin/}` and the symmetric `~/.codex/local/{,bin/}`. macOS additionally checks `~/Library/pnpm` (the macOS pnpm default), `/opt/homebrew/bin`, `/usr/local/bin`, and `/opt/local/bin` (MacPorts); Linux additionally checks `/usr/local/bin`, `/home/linuxbrew/.linuxbrew/bin`, `/opt/homebrew/bin`, `/snap/bin`, `/run/current-system/sw/bin` (NixOS system profile), and `/nix/var/nix/profiles/default/bin` (multi-user Nix).

Version-managed Node installers can't be matched by a single static path, so [[src-tauri/src/config.rs#versioned_node_bin_candidates]] walks `~/.nvm/versions/node/*/bin/` (NVM), `~/.local/share/fnm/node-versions/*/installation/bin/` (fnm), and `~/.nodenv/versions/*/bin/` (nodenv) at detection time and emits one candidate per installed version. Without this, version-manager users get a false N/A because their init scripts only run from `~/.zshrc`/`~/.bashrc`.

Windows is not covered: detection assumes a Unix shell (`bash -lc`/`zsh -lc`) and POSIX file extensions, so on Windows the login-shell lookup returns nothing and the static-path checks miss `.exe`/`.cmd`/`.ps1` shims. Provider CLI integration on Windows is intentionally unsupported until the architecture grows a Windows-native code path.

After the static list, `resolve_command_path_with_attempts` queries `npm config get prefix`, `bun pm bin -g`, and `yarn global bin` through the login shell to pick up custom global-install prefixes. Results are cached and invalidated alongside the shell PATH. Returned bin dirs are validated against a trusted-roots allow-list (`$HOME`, `/usr`, `/opt`, `/Library`, `/snap`, `/nix`, `/run/current-system`, Linuxbrew, flatpak); a malicious npm/bun config that points the prefix elsewhere is dropped before Quill could later execute the binary as a trusted CLI. Failed detections record every path inspected on `ProviderStatus.lastDetectionAttempts` (omitted from JSON when empty) with the user's home directory redacted to `~/...` so the persisted/emitted blob does not leak the local username; the integrations menu's per-row diagnostic tooltip renders the redacted paths as inline `<code>` so they read distinctly from the surrounding prose.

Claude and Codex detection share [[src-tauri/src/config.rs#detect_provider_cli]]. Pi uses the same resolver and `path_for_resolved_command` augmentation but parses the returned version to enforce its 0.84.0 floor.

## Shared Outbound HTTP Client

[[src-tauri/src/config.rs#http_client]] is the single `reqwest::Client` instance shared by every outbound HTTP call the app makes: live usage polling against the Anthropic OAuth API and the MiniMax coding-plan API in [[src-tauri/src/fetcher.rs]], and GitHub release lookups in [[src-tauri/src/releases.rs]].

The client is built with `connect_timeout(5s)` and `timeout(15s)`. Without these explicit timeouts `reqwest::Client::new()` has no upper bound on connect time and can block the `tokio` runtime indefinitely on a dead network or captive portal (see seanmonstar/reqwest#1256). The 5-second connect timeout is also the signal the poller uses to enter offline cooldown — see [[features#Features#Live Usage View]] and [[src-tauri/src/lib.rs#compute_network_backoff]].

The client is lazily initialized in a `OnceLock`, so the timeout configuration applies process-wide on first use and is reused across every poll.

## Dependencies

Key runtime and dev dependencies for both frontend and backend.

### Frontend Runtime

React 19, React DOM, Tauri API v2, the Tauri updater plugin, Sentry React 10, DOMPurify 3.4, and Marked 18.

Recharts was removed with the widget redesign — all visualization is the internal SVG kit. The process JS binding and Rust plugin were removed together; the window-state JS binding is gone while its Rust plugin remains.

### Frontend Dev

TypeScript 5.9, Vite 6.0, ESLint 9.39, @vitejs/plugin-react.

### Backend

Rust crate dependencies grouped by role. Full list in `src-tauri/Cargo.toml`.

**Core runtime**: Tauri 2, Axum 0.8, Tokio 1, rusqlite 0.31 (bundled), Tantivy 0.25, reqwest 0.13, rig-core 0.32.

**Tauri plugins**: tauri-plugin-dialog 2, tauri-plugin-single-instance 2, tauri-plugin-window-state 2, tauri-plugin-updater 2, tauri-plugin-log 2.

**Utilities**: serde/serde_json, chrono, sha2, similar 2, regex, walkdir, dirs, nix (unix only), sentry 0.34 (default-features off, with `backtrace`/`contexts`/`panic`/`reqwest`/`rustls`) for the [[features#Crash Reporting]] backend half.

**Dev-only**: serial_test 3 — used by [[src-tauri/src/data_paths.rs]] tests to serialize global env-var mutation across the three behavioral cases for each resolver (data dir, rules dir, Claude projects dir, Codex sessions dir) so concurrent test threads don't race.

**macOS-only**: objc2-app-kit 0.3, objc2-foundation 0.3, block2 0.6 — used by [[src-tauri/src/tray_keepalive.rs]] for the workaround that rebuilds the tray after sleep/wake and screen-parameter changes.
