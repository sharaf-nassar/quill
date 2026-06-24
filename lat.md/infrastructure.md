# Infrastructure

Build tooling, CI/CD pipeline, release automation, and code quality enforcement for the Quill desktop application.

## Build Configuration

The frontend uses Vite with the React plugin; the backend uses Cargo with Tauri.

### Frontend Build

Vite serves on port 8181 in dev mode and ignores `src-tauri/**` to avoid extra frontend reloads during Rust rebuilds.

Production builds target ES2020 with esbuild minification and sourcemaps. TypeScript is strict mode with ESNext modules and bundler resolution. See `vite.config.ts` and `tsconfig.json`.

### Backend Build

Rust edition 2024 uses the pinned `rust-toolchain.toml` compiler version. Crate types: `lib`, `cdylib`, `staticlib`. `build.rs` calls `tauri_build::build()`.

The bundled SQLite driver (`rusqlite` with `bundled` feature) avoids system dependency issues. Tauri bundles Claude and Codex integration assets as app resources.

### Tauri Configuration

`src-tauri/tauri.conf.json` defines product name "Quill", identifier `com.quilltoolkit.app`, with a borderless transparent main window (280x340px, min 240x200).

Bundle targets: macOS app bundle + DMG, Windows NSIS, Linux AppImage. The Linux `.deb` was dropped because Tauri's updater only self-updates AppImages, so deb installs were stranded on their installed version. The `bundle.linux.deb.desktopTemplate` (`desktop-template.desktop`) is deliberately retained even with no `.deb` shipped: the AppImage bundler builds its AppDir via the shared Debian data generator (`appimage`'s `linuxdeploy` calls `debian::generate_data`), so that template still drives the AppImage `.desktop` entry — do not remove it as "unused deb config." Auto-updater uses GitHub releases endpoint with minisign public key verification, and macOS update detection depends on shipping the signed `.app.tar.gz` updater bundle in addition to the DMG installer.

## CI/CD Pipeline

GitHub Actions workflow (`.github/workflows/release.yml`) triggers on `v*` tags or manual dispatch.

### Backend CI Gate

`.github/workflows/ci.yml` is the Rust backend gate (feature 005, FR-021 / SC-008) that also blocks release on failure.

It triggers on `pull_request`, `push` to `main`, and `workflow_call`, installs the Linux Tauri development packages before Rust setup, installs Rust from `rust-toolchain.toml`, runs in `src-tauri` with `permissions: contents: read`, and enforces `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (warnings deny — hard gate), and `cargo test`. Its single job id `rust` is stable for reuse.

`release.yml` calls it as a reusable workflow (`ci` job using `./.github/workflows/ci.yml`) and makes `create-release` `needs: ci`, so a failing learning-logic suite blocks the entire build/sign/notarize/publish chain (no OS/notarization matrix duplicated). Contract: `specs/005-learning-system-hardening/contracts/evaluation-harness.md`.

### Draft Release Pre-Creation

A `create-release` job runs before all builds to create a single draft release. This prevents a race condition where parallel `tauri-action` instances each create their own draft, splitting assets across multiple releases and breaking the updater.

### Build Matrix

Four parallel builds (fail-fast disabled), all depending on `create-release` so `tauri-action` finds the existing draft.

`tauri-action` runs with `retryAttempts: 3` because its per-build `latest.json` uploads race on the shared release asset (tauri-action#1270); the publish job rebuilds that manifest deterministically regardless (see Release Publishing below).

Platforms: Linux (Ubuntu 22.04, AppImage), macOS Intel (x86_64), macOS ARM (aarch64), Windows (NSIS, runner pinned to `windows-2025`). Each installs Node.js LTS, the pinned Rust toolchain, and platform-specific system dependencies.

### Version Injection

Parses version from git tag (e.g., `v0.2.1` -> `0.2.1`). Updates `src-tauri/Cargo.toml` via sed and `package.json` via Node.js before build.

### macOS Code Signing

Imports APPLE_CERTIFICATE from secrets into a temporary build keychain and extracts CERT_ID for codesigning.

After build, submits DMG to Apple notary service (35-minute timeout), staples the notarization ticket, and re-uploads the notarized DMG to the release.

### Release Publishing

A third job (`publish`) waits for all builds, finds the draft release, and renames assets with platform labels (e.g., `Quill_0.3.1_macOS_amd64.dmg`).

It retries the draft lookup for API eventual consistency, then rebuilds `latest.json` from scratch and publishes the release. Because `tauri-action`'s parallel per-build `latest.json` uploads race on the single shared asset and silently drop platforms (this shipped v0.3.33 with no `linux-x86_64` entry, breaking the updater for Linux), the publish job is the manifest's single writer: after renaming assets it runs `.github/scripts/assemble-latest-json.sh`, which reads each platform's signed `*.sig` asset (distinct names never race) and emits the four base updater keys (`linux-x86_64`, `darwin-aarch64`, `darwin-x86_64`, `windows-x86_64`). The script fails the release if any base platform is missing, turning a silently broken manifest into a hard failure. The macOS build still verifies that `*.app.tar.gz` plus its `.sig` exist before continuing so the `darwin-*` signatures are present to assemble.

### Required Secrets

`GITHUB_TOKEN`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`, `SENTRY_AUTH_TOKEN`.

### Sentry Source Map Upload

The Vite build uploads frontend source maps to the [[features#Crash Reporting]] Sentry project when `SENTRY_AUTH_TOKEN` is exported on the build step.

`vite.config.ts` reads `SENTRY_AUTH_TOKEN`, `SENTRY_ORG`, `SENTRY_PROJECT`, and `SENTRY_RELEASE` from `process.env` and instantiates `sentryVitePlugin` only when the token is set AND `NODE_ENV === "production"` (which Vite sets for `vite build`); dev runs and unconfigured CI jobs skip the upload silently. `release.yml`'s "Build and release" step exports `SENTRY_AUTH_TOKEN` conditionally on `matrix.platform == 'ubuntu-22.04'` so the four-platform build matrix uploads exactly once per release — the macOS/Windows builds see an empty token and the plugin no-ops there. `SENTRY_RELEASE` is set to `github.ref_name` (the `v*` tag) so source maps and the runtime SDK's `release` tag (`VITE_APP_VERSION`, same value) link to the same Sentry release.

### Pages Workflow

`.github/workflows/pages.yml` deploys the marketing site to GitHub Pages using the official `actions/deploy-pages@v4` flow.

Triggers on `push` to `main` with a paths filter on `marketing-site/**` and the workflow file itself, plus `workflow_dispatch` for manual redeploys (useful when only screenshots change). Two-job split: `build` checks out the repo, runs `actions/configure-pages@v5`, and uploads `marketing-site/` verbatim via `actions/upload-pages-artifact@v3` (no build step — the site is plain static HTML/CSS/JS); `deploy` consumes the artifact and runs `actions/deploy-pages@v4` against the `github-pages` environment so the deployed URL surfaces in the Actions UI. Permissions follow the GitHub-recommended least-privilege template: `contents:read`, `pages:write`, `id-token:write`. The `pages` concurrency group is set to `cancel-in-progress: false` to match GitHub's recommendation that an in-flight Pages deploy not be killed mid-flight. Contract: `specs/001-marketing-site/contracts/pages-workflow.md`.

### Marketing Site

The marketing site is a static GitHub Pages deliverable that sells Quill through real product screenshots and stable anchored sections.

`marketing-site/index.html` owns the single-page content and the public `#hero`, `#analytics`, `#context`, `#search`, `#live`, `#learning`, `#memory`, `#brevity`, and `#install` fragments (the original seven are a stable deep-link contract; `#memory` and `#brevity` were added 2026-06-19). `marketing-site/styles.css` owns the Signal Theater visual system: Quill's quiet dark app surface, actual logo mark, cyan/purple logo accents, clipped geometry, dense screenshot proof, and an alternating two-column spotlight rhythm that shows each lean per-section screenshot whole at its natural aspect (no cover-cropping), with self-hosted Space Grotesk (display) and Geist (body) woff2 fonts under `assets/fonts/` (OFL, preloaded, no remote fonts). `marketing-site/motion.js` adds progressive GSAP scroll-reveal (the `.motion-rise` effect) only — no pinning, scrubbed text, or carousel; the content remains readable when JavaScript is disabled or the CDN motion library fails.

The stylesheet link includes a version query so palette changes are not masked by stale browser caches during local preview or GitHub Pages deploys.

The hero is product-led and two-column on desktop: the left column holds a short problem-first headline ("Stop running your coding agents blind."), a deck line on its own row between the headline and the description carrying the bidirectional hook (you get the insight; your agents get the tools), a one-line description, install/source actions, and a trust line (own-plan, no API key, no tracking, MIT) — all left-aligned and vertically centred beside, in the right column, one slim product window (the main window with the Live usage pane stacked above the Analytics dashboard on its 7-day Now view) under a cyan/purple glow at a 400px stage cap, height-clipped to ~760px so the tall shot fades out before its session breakdown and stays compact beside the copy rather than dominating the fold. Placing the window beside the copy rather than stacked under it keeps the long shot space-efficient; under 980px the hero collapses to a single centred column (copy then window). Standalone KPI strips are avoided because the screenshot carries the evidence more credibly.
The marketing copy features Claude Code and Codex only; MiniMax was dropped from the site on 2026-06-23 (the desktop app still tracks MiniMax live limits — see [[features#Live Usage View]]). The product window peeks below the fold to invite the scroll into the lead Analytics spotlight.

The page is ordered for the narrative "analytics first, then the agent tools built on it" as a single alternating two-column spotlight rhythm — Analytics, Context/MCP (foregrounding that the agent itself calls Quill's `quill_*` tools), Search, Live, Learning, then the Memory and Brevity agent tools — each section pairing its copy with one full product screenshot on the opposite side, alternating which side the image sits on down the page, and closing on install/trust. Every screenshot is a single section captured on its own, cropped lean to its content (no window-chrome excess, no scrollbars, no dead space) and shown whole at its natural aspect rather than cover-cropped or combined with other panes. Each shot is also displayed at — or below — its native retina width via a per-section `--shot-w` custom property (default 480px; 520px for sessions/learning/memory, 560px for brevity) feeding the media grid track `minmax(0, var(--shot-w))`, so the slim product window is never upscaled wider or taller than captured and the copy column takes the remaining space. Feature copy uses short, concrete, screenshot-backed claims instead of long paragraphs, keeping the page scannable while preserving the technical details developers need.

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

The Codex path pins `gpt-5.4`, `model_reasoning_effort="xhigh"`, and `service_tier="fast"` in non-interactive mode, forces `-C` to the git repo root, and leaves Claude on its existing inference path.

When release notes are generated through Codex in an interactive terminal, `release.sh` shows a live spinner plus a framed tail of the last 20 user-meaningful Codex progress lines. Internal hook, MCP, router, and sandbox-noise lines stay hidden unless the run fails.

Prompt instructs the model to focus on user-visible features only, omitting refactors, dependency updates, and CI changes. Output format: "## What's New" section with bold feature headings.

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
| clippy | `src-tauri/**` | Platform-aware Rust linting via `scripts/precommit-rust.sh` (`-D warnings`) |
| eslint | `src/**/*.{ts,tsx}` | TypeScript linting |
| tsc --noEmit | `src/**/*.{ts,tsx}` | Type checking |

The `clippy` hook delegates to `scripts/precommit-rust.sh` (see [[infrastructure#Infrastructure#Scripts#Platform-Aware Rust Lint Hook]]) so platform-gated `#[cfg(target_os = "…")]` code is linted on a matching host instead of slipping through to the macOS Release build.

## Scripts

Utility scripts for development, testing, and documentation tasks.

### Platform-Aware Rust Lint Hook

`scripts/precommit-rust.sh` is the entry point for the `clippy` pre-commit hook. It runs `cargo clippy --all-targets -- -D warnings` against the code the host OS can compile, so platform-gated regressions surface at commit time.

`cargo clippy` only compiles for the host target triple, so `#[cfg(target_os = "…")]` code is invisible to other platforms' lint runs — a macOS-only [[src-tauri/src/lib.rs#macos_proc_pidpath]] call (`libc::proc_pidpath`) compiles clean on Linux and first breaks on the macOS Release build. The script branches on `uname -s`: macOS lints the native Apple target (covering `cfg(target_os = "macos")`); Linux lints its native target and prints a notice that macOS-gated code is not lintable locally, because objc2's build script compiles Objective-C with Apple-only clang flags (`-arch`, `-mmacosx-version-min`) that Linux `cc` rejects. Cross-linting macOS from Linux would require an osxcross toolchain plus a packaged macOS SDK. Mirrors the `--all-targets` strictness of the [[infrastructure#Infrastructure#CI/CD Pipeline#Backend CI Gate]] so the local gate is never laxer than CI.

### Screenshot Capture

`scripts/take_screenshots.sh` uses xdotool and ImageMagick to capture screenshots of all UI views. Navigates between tabs and windows by clicking titlebar buttons, generating the canonical per-section PNG files.

Default output directory is `marketing-site/assets/screenshots/` (overridable via `OUTDIR=...`). Captures use ImageMagick `import` then upscale 2× via `convert -filter Catrom -resize 200%` for HiDPI rendering on the marketing site (override with `RETINA=0`). The 2026-06-24 refresh recaptured every section on its own at a slim window width (480px main / 500–560px secondary windows) and fully populated, then cropped each shot tight to its content, replacing the earlier dual-pane shots (the main window no longer combines Live and Analytics in one frame). `hero.png` is the combined main window — the Live usage pane stacked above the Analytics dashboard on its 7-day Now view (the stacked "both" layout: Settings → Analytics panel on; captured tall at ~453px wide with fresh timestamps so neither pane shows a Paused badge; the Claude limits list is intentionally trimmed to 5h / 7d / Opus / Sonnet by deleting the `seven_day_cowork` (Code) and `seven_day_oauth_apps` (OAuth) `usage_snapshots` rows before the shot and re-seeding to restore them afterward — the `#live` section's `live.png` still shows all six); `analytics-charts.png` is the Charts view (token/code/cache timelines, shown at `#analytics`); `live.png` is the Live rate-limit pane alone (both Claude and Codex, no error banner or Paused badge); plus `analytics-context.png` (Context tab — reachable when context preservation is enabled and `context_savings_events` rows exist; the seeder populates both), `sessions.png` (Session Search with a result detail open), `learning.png` (Learning panel — five active rules above three discovered candidates plus the domain breakdown, no empty section), and the two new agent-tool shots `memory.png` (Memories panel showing a clean "All Projects (4)") and `brevity.png` (Brevity toggle, MiniMax excluded from the visible copy). Capture filenames map 1:1 to the marketing site's anchored sections per `specs/001-marketing-site/data-model.md`; the page then displays each at or below its native retina width (see [[infrastructure#CI/CD Pipeline#Marketing Site]]).

That refresh was captured against the packaged AppImage build under an isolated `HOME`, not via `take_screenshots.sh` directly: on the maintainer's GNOME/Mutter (X11) setup the raw `target/{debug,release}` binaries render through a GL surface ImageMagick `import` cannot read (only the AppImage-bundled WebKit is grabbable), and the debug binary loads the dev-server URL so it renders blank. The isolated `HOME` also keeps the demo's provider-enable, brevity-block, and memory writes off the maintainer's real `~/.claude` / `~/.codex`. Folding this AppImage + isolated-`HOME` setup (plus a seeded Codex usage transcript under `$HOME/.codex/sessions/` so the Live Codex bars resolve, and four seeded memory files — one per project — under `$HOME/.claude/projects/<slug>/memory/`) into `run_quill_demo.sh` for one-command reproducibility is pending. The Memories panel's "All Projects (N)" counts every per-project context file, which includes the global `~/.claude/CLAUDE.md` that `claude_setup` recreates on each launch (counted once per project — see [[src-tauri/src/memory_optimizer.rs#get_known_projects]]); to capture a clean "All Projects (4)" that file is removed after launch but before the Memories panel is first opened (the panel counts on window mount, and `claude_setup` does not run again until the next launch). The seeder's `PROJECTS` deliberately use single-segment paths (e.g. `/home/alex/gateway`, never `/home/alex/api-gateway`) because the panel's slug↔path round-trip ([[src-tauri/src/memory_optimizer.rs#project_path_to_slug]] encodes `/`→`-`, [[src-tauri/src/memory_optimizer.rs#slug_to_path]] decodes `-`→`/`) cannot tell an internal dash from a path separator; single-segment names keep the round-trip unambiguous so the panel shows clean, de-duplicated project headings.

### Dummy Data Seeder

`scripts/populate_dummy_data.py` seeds the SQLite database with deterministic, realistic sample data across every analytics table, and stamps it at the app's current schema version so a freshly-seeded DB opens with zero migrations.

By default, checks that Quill is not running before modifying the personal DB to prevent WAL corruption, creates a backup before seeding, and writes sample rule files to `~/.claude/rules/learned/`. Supports an optional sandbox mode for the marketing-site screenshot pipeline: `--data-dir PATH` redirects the SQLite file under `PATH/usage.db` and skips the running-Quill guard (a sandbox can coexist with a personal Quill since they target different files); `--rules-dir PATH` redirects the sample rule files; `--projects-dir PATH` writes fictional Claude session JSONL files (one project subdir per `PROJECTS` entry, two `<sessionId>.jsonl` files each) so Session Search has indexable content; `--no-projects` skips that step; `--no-backup` skips the existing-DB backup; `--seed INT` overrides the RNG seed (default `42`); `--quiet` suppresses per-step progress while keeping the final summary. The seeder builds the complete current-version schema up front — every table the app expects, including `session_events`, `skill_usages`, `hook_invocations`, and the migration-25 rule-versioning tables — and records `schema_version` 1 through the latest, so the app runs no migrations on open. (Recording only the legacy versions while building final-shape tables crashed a fresh app on a later `ALTER ... ADD COLUMN`.) It populates `session_events` (the exclusive source for the redesigned LLM-runtime card), a last-hour activity cluster plus 30-day history, rate-limit utilization on the app's 0–100 percent scale (storing 0–1 fractions rendered every live bar at ~0%), and `source_ref`-linked preservation/retrieval events so context-reuse reads nonzero. The latest `usage_snapshots` row per bucket is stamped with a near-future RFC3339 timestamp (`ts_tz`, NOW + 30 min) rather than a naive one, so the backend's recent-snapshot freshness check (`parse_from_rfc3339`, the "Path A" branch) serves the live bars straight from the DB and skips the live provider fetch — keeping the demo Live view off the Paused/offline state for both Claude and Codex (Codex also needs a seeded usage transcript under `$HOME/.codex/sessions/`, written at capture time). Confirmed learned rules are written into the provider-scope subdir the app actually scans in demo mode (`<rules-dir>/claude/`) so they read as active, while emerging rules stay in `<rules-dir>` alone and read as discovered candidates — a rule is active only when its `.md` sits in the scanned scope dir ([[src/types.ts#isActiveRule]]), so this split gives the Learning view a populated ACTIVE RULES section beside DISCOVERED instead of an empty one. It also self-renders the demo by seeding settings that enable the Claude and Codex providers (`integration.providers.v1`, with MiniMax omitted), set a roomy window size, turn on the brevity profile, and decline the AppImage first-run prompt (`appimage.integration`), so the sandbox shows fully populated views with no manual setup. Full CLI surface in `specs/001-marketing-site/contracts/seeder-cli.md`.

### Demo Launcher

`scripts/run_quill_demo.sh` (POSIX) and `scripts/run_quill_demo.ps1` (Windows) launch a sandboxed Quill instance against dummy data without touching the maintainer's personal Quill state.

Each launcher creates a stable per-user sandbox directory (`/tmp/quill-demo-$USER` on POSIX, `%TEMP%\quill-demo-%USERNAME%` on Windows), exports `QUILL_DEMO_MODE=1` plus `QUILL_DATA_DIR=$SANDBOX/data`, `QUILL_RULES_DIR=$SANDBOX/rules`, and `QUILL_CLAUDE_PROJECTS_DIR=$SANDBOX/projects` (which engages the [[backend#Data Paths#Demo-mode path override]]), runs the seeder against the sandbox with `--no-backup --quiet`, then auto-discovers the Quill binary (override flag, `$PATH`, `target/release/quill`, `target/debug/quill`) and execs it. The Codex sessions resolver short-circuits to an empty `<TMP>/quill-demo-empty-codex-sessions/` placeholder when demo-mode is on without `QUILL_CODEX_SESSIONS_DIR` set, so the demo Quill never indexes the maintainer's real `~/.codex/sessions/` even though the launcher only seeds Claude. Flags: `--clean` / `-Clean` wipes the sandbox before launch, `--bin PATH` / `-Bin PATH` overrides binary discovery, `--keep-on-exit` / `-KeepOnExit` suppresses the teardown-command hint. Full CLI surface in `specs/001-marketing-site/contracts/launcher-cli.md`.

### macOS Bootstrap

`scripts/mac.sh` bootstraps a macOS 14+ machine by installing Homebrew with the current official installer, then refreshing Homebrew metadata before installing or upgrading the moving `node` formula and `docker-desktop` cask.

The script exits early on non-macOS hosts and unsupported macOS releases so failures are explicit. It treats Docker as Docker Desktop on macOS because that installs the app/runtime rather than only the standalone `docker` client binary.

## Claude Integration Deployment

Claude integration now has a dedicated adapter shell in
[[src-tauri/src/integrations/claude.rs]] plus manifest-aware install/uninstall wrappers in
[[src-tauri/src/claude_setup.rs]], while startup detection remains in
[[src-tauri/src/integrations/manager.rs]].

`Task 3A` introduces manifest generation during setup so uninstall can remove only owned artifacts:
hook entries marked with [[src-tauri/src/claude_setup.rs#HOOK_MARKER]],
`mcpServers.quill` from `~/.claude.json`, and the `[[src-tauri/src/claude_setup.rs#BLOCK_START]]`/`[[src-tauri/src/claude_setup.rs#BLOCK_END]]` managed block in `CLAUDE.md`.
Migration from legacy heading-based detection to block markers happens automatically on first run.
`confirm_enable_provider` now routes Claude through that adapter install path, and
[[src-tauri/src/restart.rs#uninstall_claude_restart_assets]] removes Quill-owned restart hooks,
shell integration lines, and cache files when Claude uninstall runs.

### Deployed Assets

Files and directories created during first-launch auto-deployment.

| Target | Content |
|--------|---------|
| `~/.config/quill/scripts/` | Base hook scripts for token reporting, session sync, and qbuild edit guarding. `observe.cjs` is added when activity tracking is enabled (default on). Context routing and continuity capture are added when context preservation is enabled, plus `context-telemetry.cjs` when context telemetry is also on (default on, gated on context preservation) |
| `~/.config/quill/mcp/` | Python MCP server for session querying; working-context tools only when context preservation is enabled |
| `~/.claude/commands/` | Custom CLI commands (if applicable) |
| `~/.claude/settings.json` | Hook registrations (marked with `_source: "quill-setup"`) |
| `~/.claude.json` | MCP server registration |
| `~/.claude/CLAUDE.md` | Quill MCP usage instructions injected as a managed block between `<!-- quill-managed:claude:start -->` / `<!-- quill-managed:claude:end -->` markers |

Scripts get 0o755 permissions; auth files get 0o600. Existing hook entries are detected to avoid duplication. Original `settings.json` is backed up before patching. Deployed `observe.cjs`, `session-sync.cjs`, and optional `context-telemetry.cjs` calls cap HTTP waits so provider CLIs fail open instead of timing out on Quill stalls.

Claude always installs the qbuild guard PreToolUse hook and the Stop hook for token reports and session sync. PreToolUse and PostToolUse `observe.cjs` hooks ride with the activity tracking flag (see [[features#Settings Window#Integration Features]]) so privacy-conscious users can keep token stats but skip live tool-call telemetry. When context preservation is enabled, Claude additionally installs SessionStart, UserPromptSubmit, PreCompact, PreToolUse, and Stop context hooks. Context hooks call `src-tauri/claude-integration/scripts/context-router.cjs` and `src-tauri/claude-integration/scripts/context-capture.cjs`, both of which try to load `context-telemetry.cjs` for context-savings events when that flag is also on (and tolerate its absence otherwise). Startup repair in [[src-tauri/src/integrations/manager.rs]] reads the full `IntegrationFeatures` set and reinstalls every enabled provider with the merged feature set.

## Codex Integration Deployment

Codex integration lives in [[src-tauri/src/integrations/codex.rs]] and deploys provider-specific assets under `~/.config/quill/codex/` plus Quill-managed entries in the user's Codex home.

### Deployed Assets

Files and config entries created when the Codex provider is enabled.

Deployment is allowlisted to token and sync scripts by default. `observe.cjs` and `hook-observe.cjs` are added when activity tracking is enabled. Context routing and continuity scripts are deployed only when context preservation is enabled, with `context-telemetry.cjs` further gated on the context telemetry flag. The Claude-only `qbuild-guard.sh` is never copied into Codex assets.

| Target | Content |
|--------|---------|
| `~/.config/quill/codex/scripts/` | Base hook scripts for token reporting and session sync. `observe.cjs` is added when activity tracking is enabled (default on); `hook-observe.cjs` rides with the same flag and ships hook-fire telemetry for the Now-tab Hooks breakdown via `POST /api/v1/hooks/observed`. Context routing and continuity capture are added when context preservation is enabled, plus `context-telemetry.cjs` when context telemetry is also on |
| `~/.config/quill/codex/mcp/` | Python MCP server copied from the bundled Quill MCP assets; working-context tools only when context preservation is enabled |
| `~/.config/quill/codex/templates/` | Managed AGENTS template block |
| `~/.codex/config.toml` | `features.hooks = true`, inline `[[hooks.*]]` Quill hook registrations, Codex `hooks.state` trust hashes, plus a Quill-managed `mcp_servers.quill` block when no manual entry exists |
| `~/.codex/AGENTS.md` | Managed Quill session-history guidance block |

Codex install and uninstall remove only Quill-owned legacy `hooks.json` commands and delete `hooks.json` when no non-Quill hooks remain, then remove related hook trust state, managed config blocks, AGENTS blocks, and provider-owned asset directories. Codex deploys the same bounded-wait observation and session-sync behavior as Claude so a slow local widget cannot hold Codex hooks open until the host kills them.

Codex installs SessionStart, UserPromptSubmit, and Stop hooks unconditionally; PreToolUse and PostToolUse `observe.cjs` hooks ride with the activity tracking flag. When context preservation is enabled, Codex also installs SessionStart, UserPromptSubmit, PreToolUse, PreCompact, and Stop context hooks. When activity tracking is enabled, the installer additionally registers `hook-observe.cjs` on every one of the eight Codex hook events (`PreToolUse`, `PostToolUse`, `SessionStart`, `UserPromptSubmit`, `Stop`, `PreCompact`, `PostCompact`, `PermissionRequest`) with no matcher, so the Now-tab Hooks breakdown sees every fire — Codex rollouts do not log hook executions, so this observer is the only source of Codex hook data. The installer asks `codex app-server` for `hooks/list` metadata, then writes each Quill hook's `trusted_hash` through `config/batchWrite` so the trust state matches Codex's own hook-review model.

The app-server request pipe is flushed but kept open until each response arrives. Closing stdin immediately after writing requests can make `hooks/list` return no response before trust hashes are written.

Quill resolves the Codex CLI before running provider checks or `codex app-server`, then augments the child process `PATH` with launcher and symlink-target directories so Homebrew and npm installs work from macOS app launches with stripped inherited environments.

## Provider CLI Detection

Claude and Codex CLI detection runs through [[src-tauri/src/config.rs#resolve_command_path]] with an invalidatable login-shell PATH cache so the integrations menu's "Rescan PATH" action can pick up new installs without restarting Quill.

Detection layers a login-shell `command -v` lookup with a static fallback list and dynamic per-package-manager prefix queries. The cache lives in an `RwLock` and is cleared via [[src-tauri/src/config.rs#refresh_shell_path]] when the UI calls [[src-tauri/src/integrations/manager.rs#force_rescan]].

The static fallback list covers per-user package managers that frequently aren't in the login-shell PATH because users add them only to interactive shell config (`~/.zshrc`, `~/.bashrc`) which `zsh -lc` does not source: `~/.bun/bin`, `~/.cargo/bin`, `~/.deno/bin`, `~/.volta/bin`, `~/.local/bin`, `~/.local/share/pnpm`, `~/.npm-global/bin`, `~/n/bin`, `~/.yarn/bin`, `~/.config/yarn/global/node_modules/.bin`, `~/.nix-profile/bin`, `~/.asdf/shims`, `~/.nodenv/shims`, `~/.local/share/mise/shims`, plus Anthropic's `claude migrate-installer` target `~/.claude/local/{,node_modules/.bin/}` and the symmetric `~/.codex/local/{,bin/}`. macOS additionally checks `~/Library/pnpm` (the macOS pnpm default), `/opt/homebrew/bin`, `/usr/local/bin`, and `/opt/local/bin` (MacPorts); Linux additionally checks `/usr/local/bin`, `/home/linuxbrew/.linuxbrew/bin`, `/opt/homebrew/bin`, `/snap/bin`, `/run/current-system/sw/bin` (NixOS system profile), and `/nix/var/nix/profiles/default/bin` (multi-user Nix).

Version-managed Node installers can't be matched by a single static path, so [[src-tauri/src/config.rs#versioned_node_bin_candidates]] walks `~/.nvm/versions/node/*/bin/` (NVM), `~/.local/share/fnm/node-versions/*/installation/bin/` (fnm), and `~/.nodenv/versions/*/bin/` (nodenv) at detection time and emits one candidate per installed version. Without this, version-manager users get a false N/A because their init scripts only run from `~/.zshrc`/`~/.bashrc`.

Windows is not covered: detection assumes a Unix shell (`bash -lc`/`zsh -lc`) and POSIX file extensions, so on Windows the login-shell lookup returns nothing and the static-path checks miss `.exe`/`.cmd`/`.ps1` shims. Provider CLI integration on Windows is intentionally unsupported until the architecture grows a Windows-native code path.

After the static list, `resolve_command_path_with_attempts` queries `npm config get prefix`, `bun pm bin -g`, and `yarn global bin` through the login shell to pick up custom global-install prefixes. Results are cached and invalidated alongside the shell PATH. Returned bin dirs are validated against a trusted-roots allow-list (`$HOME`, `/usr`, `/opt`, `/Library`, `/snap`, `/nix`, `/run/current-system`, Linuxbrew, flatpak); a malicious npm/bun config that points the prefix elsewhere is dropped before Quill could later execute the binary as a trusted CLI. Failed detections record every path inspected on `ProviderStatus.lastDetectionAttempts` (omitted from JSON when empty) with the user's home directory redacted to `~/...` so the persisted/emitted blob does not leak the local username; the integrations menu's per-row diagnostic tooltip renders the redacted paths as inline `<code>` so they read distinctly from the surrounding prose.

Both Claude and Codex detection share [[src-tauri/src/config.rs#detect_provider_cli]], which calls `resolve_command_path_with_attempts`, runs `--version` with `path_for_resolved_command`'s symlink-aware PATH augmentation, and returns the success bool plus the (already-redacted) attempts list.

## Shared Outbound HTTP Client

[[src-tauri/src/config.rs#http_client]] is the single `reqwest::Client` instance shared by every outbound HTTP call the app makes: live usage polling against the Anthropic OAuth API and the MiniMax coding-plan API in [[src-tauri/src/fetcher.rs]], and GitHub release lookups in [[src-tauri/src/releases.rs]].

The client is built with `connect_timeout(5s)` and `timeout(15s)`. Without these explicit timeouts `reqwest::Client::new()` has no upper bound on connect time and can block the `tokio` runtime indefinitely on a dead network or captive portal (see seanmonstar/reqwest#1256). The 5-second connect timeout is also the signal the poller uses to enter offline cooldown — see [[features#Features#Live Usage View]] and [[src-tauri/src/lib.rs#compute_network_backoff]].

The client is lazily initialized in a `OnceLock`, so the timeout configuration applies process-wide on first use and is reused across every poll.

## Dependencies

Key runtime and dev dependencies for both frontend and backend.

### Frontend Runtime

React 19, React DOM, Tauri API v2, Tauri plugins (updater, window-state, process), Recharts 3.7, DOMPurify 3.4, and Marked 18.

### Frontend Dev

TypeScript 5.9, Vite 6.0, ESLint 9.39, @vitejs/plugin-react.

### Backend

Rust crate dependencies grouped by role. Full list in `src-tauri/Cargo.toml`.

**Core runtime**: Tauri 2, Axum 0.8, Tokio 1, rusqlite 0.31 (bundled), Tantivy 0.25, reqwest 0.13, rig-core 0.32.

**Tauri plugins**: tauri-plugin-dialog 2, tauri-plugin-single-instance 2, tauri-plugin-window-state 2, tauri-plugin-updater 2, tauri-plugin-process 2, tauri-plugin-log 2.

**Utilities**: serde/serde_json, chrono, sha2, parking_lot 0.12, similar 2, regex, walkdir, dirs, nix (unix only), sentry 0.34 (default-features off, with `backtrace`/`contexts`/`panic`/`reqwest`/`rustls`) for the [[features#Crash Reporting]] backend half.

**Dev-only**: serial_test 3 — used by [[src-tauri/src/data_paths.rs]] tests to serialize global env-var mutation across the three behavioral cases for each resolver (data dir, rules dir, Claude projects dir, Codex sessions dir) so concurrent test threads don't race.

**macOS-only**: objc2-app-kit 0.3, objc2-foundation 0.3, block2 0.6 — used by [[src-tauri/src/tray_keepalive.rs]] for the workaround that rebuilds the tray after sleep/wake and screen-parameter changes.
