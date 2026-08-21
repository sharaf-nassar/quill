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

Rust edition 2024 uses the pinned `rust-toolchain.toml` compiler version. The library uses Cargo's default Rust library output for the desktop binary. `build.rs` calls `tauri_build::build()`.

The `quill` app binary is Cargo's default run target; maintenance spikes under
`src-tauri/src/bin/` remain explicit `cargo run --bin <name>` targets.

The bundled SQLite driver (`rusqlite` with `bundled` feature) avoids system dependency issues. Tauri bundles Claude and Codex integration assets as app resources.

### Tauri Configuration

`src-tauri/tauri.conf.json` defines product name "Quill", identifier `com.quilltoolkit.app`, with a borderless transparent main window (280x340px, min 240x200).

Bundle targets: macOS app bundle + DMG, Windows NSIS, Linux AppImage. The Linux `.deb` was dropped because Tauri's updater only self-updates AppImages, so deb installs were stranded on their installed version. The `bundle.linux.deb.desktopTemplate` (`desktop-template.desktop`) is deliberately retained even with no `.deb` shipped: the AppImage bundler builds its AppDir via the shared Debian data generator (`appimage`'s `linuxdeploy` calls `debian::generate_data`), so that template still drives the AppImage `.desktop` entry — do not remove it as "unused deb config." Auto-updater uses GitHub releases endpoint with minisign public key verification, and macOS update detection depends on shipping the signed `.app.tar.gz` updater bundle in addition to the DMG installer.

#### Development Identity

A development run claims `com.quilltoolkit.app.dev` so it cannot mutate the installed app's data. It shares the installed app's listener and provider contract — see [[backend#Backend#Data Paths#Development runtime isolation]] — so the two cannot run at the same time.

`src-tauri/tauri.dev.conf.json` overrides `identifier` and the main window's initial `focus` (`false`), and `scripts/tauri.mjs` — the `npm run tauri` entry point — appends `--config src-tauri/tauri.dev.conf.json` when the subcommand is `dev` and the caller passed no `--config`/`-c` of their own. Every development identity repairs enabled provider integrations, including Pi, unless `QUILL_DEV_INTEGRATIONS=0` opts out. `build` and every other subcommand pass through untouched, so release output is byte-identical. The Tauri CLI hands the merged config to the build through `TAURI_CONFIG`, so `tauri::generate_context!()` embeds the dev identifier and the isolation survives `tauri dev --release` — `debug_assertions` deliberately plays no part.

`focus: false` keeps every Rust file-watch rebuild — a brand-new process, not a reload — from stealing OS focus while someone is mid-edit; the production window keeps Tauri's default (focused) since only `tauri.conf.json` ships. Tauri merges `--config` as an RFC 7396 JSON Merge Patch, which replaces arrays wholesale instead of merging them element-wise, so the dev window block restates every field from the release window rather than just adding `focus`.

The identifier is the single identity that Tauri's own per-app directories (log dir, window state), the `tauri-plugin-single-instance` D-Bus name / named mutex, and every Quill-owned data path derive from; see [[backend#Backend#Data Paths#Development path isolation]]. `scripts/tauri-dev-identity.test.mjs` pins the two identifiers, cross-checks that the dev window mirrors the release window in every field except `focus`, and pins the dev/build argument split.

## CI/CD Pipeline

GitHub Actions workflow (`.github/workflows/release.yml`) triggers on `v*` tags or manual dispatch.

Manual dispatch must select an existing `v*` tag. The `create-release` job rejects branch refs before checkout or draft creation, so version injection and packaging remain tag-only.

### Backend CI Gate

`.github/workflows/ci.yml` is the cross-platform Rust backend gate that also blocks release on failure.

It triggers on `pull_request`, `push` to `main`, and `workflow_call`, runs in `src-tauri` with `permissions: contents: read`, and pins Rust 1.95.0 plus the Cargo cache. The Linux job installs Tauri development packages, provisions uv, and runs `cargo test` through the locked MCP Python project so Rust/Python parity tests receive the packaged dependencies; it also enforces `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`. The `macos-latest` job keeps `cargo check --all-targets` for AppKit-only code and runs the focused `runtime_backfill_` tests against bundled SQLite in the macOS filesystem/runtime environment before merge.

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

`marketing-site/index.html` owns the single-page content and public `#hero`, `#analytics`, `#models`, `#context`, `#search`, `#live`, `#learning`, `#memory`, `#brevity`, `#integrations`, and `#install` fragments. The original seven remain the stable contract; the other four are additive. `marketing-site/styles.css` owns the Signal Theater visual system and whole-image spotlight rhythm, with self-hosted Space Grotesk and Geist. `marketing-site/motion.js` adds progressive `IntersectionObserver` reveals; reduced-motion and no-JavaScript clients keep readable content.

The stylesheet link includes a version query so palette changes are not masked by stale browser caches during local preview or GitHub Pages deploys.

The hero is product-led and two-column on desktop: problem-first copy and install/source actions sit beside the whole 360×800 Usage widget. Its trust line states local data, no Quill account, and opt-out scrubbed crash reports. Under 980px it collapses to one centered column. The current marketing composition spotlights Claude Code, Codex, and Pi; MiniMax remains supported in-product but is omitted from these screenshots and lead copy.

The narrative is Usage, Models, Context, Search, Live, Learning, Memory, Brevity, Integrations, then install/trust. Widget assets are 720×1600 retina captures displayed at 360px; full Tools assets are 1920×1360, while Integrations is 1920×1020 after the Pi row. Every image is shown whole except `#live`, whose `.shot-band` clips the shared Usage frame to LIMITS. Copy states evidence gaps, review-gated rule promotion, provider scope, and crash-report opt-out.

The visual contract is documented in `marketing-site/README.md` and `specs/001-marketing-site/spec.md`; screenshot assets come from the isolated Docker workflow under [[infrastructure#Scripts#Screenshot Capture]].

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
| ------ | ------- | --------- |
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

`npm test` runs every `scripts/*.test.mjs` file through Node's native, quoted glob handling, serialized with `--test-concurrency=1` because Vite-backed test files share process-external optimizer/HMR state (the `node_modules/.vite` cache directory and the default HMR port 24678), which concurrent test processes corrupt or contend over. Focused verification can invoke a test file directly with `node --test`.

### Screenshot Capture

`scripts/capture_screenshots_docker.sh` is the publishing entry point: it builds the current Tauri release, runs a fictional-data Quill in a private Xvfb desktop, and replaces canonical PNGs only after the complete capture succeeds.

`Dockerfile.screenshots` supplies Debian 12, WebKitGTK, Xvfb, Openbox, xcompmgr, xdotool, ImageMagick, Python, Node, the pinned Rust toolchain, and a harmless `pi --version` stub so the fictional Pi home renders as enabled. The release build enables `tauri/custom-protocol`; without it the binary retains `devUrl` and paints an empty window. Runtime uses software rendering plus disabled WebKit DMA-BUF/compositing paths, Docker networking `none`, no host display socket, and no personal data mount. `docker cp` exports only validated PNGs, so a failed run leaves tracked assets untouched.

Inside that desktop, `scripts/take_screenshots.sh` drives the real Tauri windows. The widget menu remains keyboard/pixel-probe driven; fixed pointer targets are limited to deterministic 360×800 widget controls and 960×680 Tools subviews whose DOM has no external automation API.

Widget view selection stays keyboard-first because LIMITS height moves the dropdown: Tab plus ArrowDown probes for the raised `#1b212b` listbox, then Home + ArrowDown ×N + Enter selects the `VIEWS` row. Native tab order selects 6H or 7D while Usage retains its default model grouping. Tools sections use Ctrl+K labels. Session result selection, Memories, Integrations, Context, and the Usage Skills breakdown use window-relative targets on fixed container geometry. Pointer parking and focus drops keep hover, tooltips, and `:focus-visible` rings out of captures.

Output is `hero.png` and its `live.png` copy, `models.png`, `analytics-context.png`, `sessions.png`, `learning.png`, `memory.png`, `settings.png`, and `brevity.png`. Tools capture first so Session Search completes retained transcript synchronization before the widget shots. Usage is fixed to 6H, keeps model grouping, and selects Skills so Claude/Codex/Pi counts are visible; Models uses 7D; Context returns to 6H. `settings.png` crops after Pi to omit MiniMax. Every grab is checked for nonblank output, stripped, compressed, and upscaled 2×.

The inner driver defaults to `marketing-site/assets/screenshots/` and still accepts `OUTDIR`/`RETINA` for debugging. The Docker wrapper captures to `/output`, checks the nine-file contract plus 720×1600 widget, 1920×1360 Tools, and 1920×1020 Integrations dimensions, copies into a host temporary directory, then installs each canonical file.

The 2026-08-21 refresh is the first fully container-owned set. Widget captures are 720×1600 and Tools captures are 1920×1360. The root `README.md` and marketing site embed the same canonical files; no mirrors are maintained. Host capture remains a debugging fallback, but it can steal focus and select another visible `Quill` window, so it is not the publishing path.

The seeded dataset is photographed as-seeded; composition grooming belongs to [[infrastructure#Scripts#Dummy Data Seeder]]. The selected Quill executable creates or migrates `usage.db`, so schema authority stays in Rust. Claude sessions use Sonnet, Opus, and Haiku across staggered six-hour peaks; Codex uses Terra, Sol, and Luna; watcher-side Pi sources add Gemini plus routed Claude/Codex models. The uncapped 1,001-id stress source is ten days old so it cannot flood photographed ranges. Four seeded skills carry explicit counts for all three CLIs. MiniMax is disabled, Pi is enabled, and `legacy_rules_archived=1` protects current fixture rules from the one-time production archive.

The automated compositions are load-bearing: Session Search must show `parser` plus detail; Learning must show active rules above candidates; Memories must show `All Projects (4)`; Integrations must show Claude/Codex/Pi without MiniMax; Context must show Brevity ON; Usage must show 6H model curves and non-zero Skills rows for every agent CLI. The launcher's watcher still removes the recreated global `CLAUDE.md` before Memories mounts. Single-segment fictional project paths preserve the panel's slug↔path round-trip.

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

## Shared Provider Config Contract

Claude, Codex, and Pi share one local server contract so every provider can be enabled independently without a silent missing-config install.

[[src-tauri/src/integrations/config_contract.rs#write_local_contract]] writes `~/.config/quill/config.json` with `url`, `context_url`, `hostname`, and `secret`, using the same `QUILL_PORT` and `QUILL_CONTEXT_PORT` resolution as [[src-tauri/src/server.rs#start_server]]. Local repair refreshes all owned fields, preserves unknown fields, and leaves deliberate remote URLs untouched. The file is mode `0600` on Unix.

Every Claude, Codex, and Pi enable path calls the shared writer. Pi includes the file in its snapshot transaction and semantic verification, so a Pi-only enable is complete and startup repair heals local port, hostname, or secret drift. [[src-tauri/src/integrations/manager.rs#should_remove_shared_config]] keeps the file until the last enabled Claude, Codex, or Pi provider is disabled; service-only providers do not extend its lifetime.

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
| -------- | --------- |
| `~/.config/quill/scripts/` | Base CJS hook scripts for token reporting, session sync, and qbuild edit guarding. `observe.cjs` is added when activity tracking is enabled (default on). Context routing is added when context preservation is enabled, plus `context-telemetry.cjs` when context telemetry is also on (default on, gated on context preservation) |
| `~/.config/quill/mcp/` | Python MCP server for session querying; working-context tools only when context preservation is enabled |
| `<Claude config>/commands/` | Custom CLI commands; `<Claude config>` is `CLAUDE_CONFIG_DIR` or `~/.claude` |
| `<Claude config>/settings.json` | Hook registrations marked with advisory `_source: "quill-setup"` or `quill-context-preservation` metadata |
| `<Claude config>/.claude.json` | MCP server registration when `CLAUDE_CONFIG_DIR` is set; the default installation keeps Claude's legacy `~/.claude.json` location |
| `<Claude config>/CLAUDE.md` | Quill MCP usage instructions injected as one exact managed block between `<!-- quill-managed:claude:start -->` / `<!-- quill-managed:claude:end -->` markers. The base template drops the working-context pointer when context preservation is off |
| `~/.config/quill/claude/integration-state.json` | Mode-`0600` path and ownership state plus the captured prior `mcpServers.quill` value |

Managed CJS files are data files, not executables. Hook entries use Claude's exec form with an absolute Node executable in `command` and the absolute script path in `args`; qbuild receives the resolved Git executable as a second arg. Setup requires Node.js 18+ and Git before mutation. The ownership state is mode `0600` on Unix.

Deployment stamps hash the bundled managed assets and enabled features. Any changed managed asset makes an installation stale; the next [[src-tauri/src/integrations/manager.rs#repair_provider]] reinstalls it and refreshes the stamp without a version bump.

[[src-tauri/src/claude_setup.rs#remove_matching_hook_handlers]] removes only exact Quill command handlers from each matcher group's nested `hooks` array. Third-party siblings, matchers, and unknown group metadata survive; only groups and event keys emptied by owned removal are pruned. Ownership uses exact exec-form command/args identities plus explicit retired shell-form paths, never a serialized-group substring. `_source` remains advisory because Claude may discard unknown fields.

[[src-tauri/src/claude_setup.rs#verify_hook_settings]] requires one exact copy of every expected group and the exact total number of managed handlers, including command, args, matcher, and timeout. Verification also requires the expected MCP object, one current instruction block, complete ownership state, and feature-consistent asset presence before commit.

### Hook Event Matrix

Claude hook coverage is explicit and limited to events that drive Quill behavior.

| Gate | Event / matcher | Handlers and timeout |
| ------ | ----------------- | ---------------------- |
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
| -------- | --------- |
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

[[src-tauri/src/integrations/codex.rs#run_app_server_request]] is the single one-shot `codex app-server` path. Hook registration and usage polling differ in feature, identity, `CODEX_HOME`, provider isolation, and deadline.

Hook discovery selects the built-in `ollama` provider through a process-only config override. This prevents `hooks/list` from refreshing unrelated model-provider auth without changing the user's `model_provider`, `model_providers`, bearer token, base URL, or auth settings. Usage polling applies no provider override because `account/rateLimits/read` needs the configured OpenAI account.

Each call spawns the CLI, sends `initialize`, `initialized`, and one request at id 2, then reads stdout until that id answers or the caller's deadline expires. Hook work uses ten seconds because it runs at startup holding the process-wide mutation guard; usage polling uses thirty because the child round-trips to the ChatGPT backend.

Teardown is the load-bearing part. The `codex` entry point is typically an npm wrapper that re-execs the platform binary as a grandchild, so signalling only the direct child orphans a process that still holds the stdout pipe open. [[src-tauri/src/integrations/codex.rs#ReapedChild#spawn]] therefore puts the child in its own process group and [[src-tauri/src/integrations/codex.rs#ReapedChild#terminate]] signals that group before reaping — signalling first, because `wait` frees the pid for reuse.

The reader thread is deliberately never joined. Group termination closes the pipe, so it ends on its own; the only case a join would cover is a child that escaped the group, and that is precisely the case where joining blocks the caller — and the mutation guard it holds — instead of returning the timeout already computed. A leaked reader is bounded; a wedged startup repair silently freezes every later provider mutation.

Process groups are a `#[cfg(unix)]` mechanism. Windows is not a supported target, and the unconditional deadline plus non-blocking teardown keep the failure there bounded to a leaked child rather than a wedge.

## Pi Integration Deployment

[[src-tauri/src/integrations/pi.rs]] owns Pi detection, extension deployment, managed instructions, transactional removal, and startup repair.

Pi must resolve through the shared login-shell, launcher, and symlink-aware PATH logic and report version 0.84.0 or newer. A resolved CLI with old or unparseable output, invalid persisted paths, or a non-writable extensions directory produces an error status with `last_error`; saved status merging preserves that fresh failure.

The selected `$PI_CODING_AGENT_DIR` and `$PI_CODING_AGENT_SESSION_DIR`, or their `~/.pi/agent` defaults, are captured in `~/.config/quill/pi/integration-state.json` with the detected Pi version. Later repair and removal use those persisted paths even if the environment changes.

Installation copies the bundled `quill.ts` to `<Pi config>/extensions/quill.ts` and maintains one Quill block in `<Pi config>/AGENTS.md`. The extension payload marker and managed block markers define ownership. A user-owned `quill.ts` blocks installation; unrelated extension files and user instruction bytes remain untouched. Disabling removes marked Quill files, the managed block, state, stamp, and bounded extension log, then gates lifecycle ingestion. It does not delete indexed Pi sessions or analytics. Legacy spool cleanup proceeds after persisted-source reconciliation without waiting for a reporter-generation acknowledgement.

Pi's npm package is user-owned. Quill never edits Pi package settings or npm storage, while `pi install`, `pi update`, `pi config`, and `pi remove` own that lifecycle. Managed uninstall cannot remove the package, and package removal cannot remove Quill's managed file or local data.

Pi uses [[src-tauri/src/integrations/deploy.rs#FileSnapshots]] as a configuration-only transaction over individual files, including the shared provider contract. The same transaction snapshots and restores context-listener and lifecycle-ingestion settings, so config, database gates, deployed bytes, instruction/state files, and stamp roll back together. It never stages or renames the extensions directory, so sibling extensions cannot enter Quill's backup. The global mutation guard invokes Pi recovery before every provider mutation.

Startup repair takes the same fast path as Codex: a deployment is current only when its bundled-source stamp matches and semantic verification passes. An old stamp therefore triggers the idempotent install path and replaces the owned extension with current bundled bytes without user action. Verification checks exact extension bytes, payload ownership, the current AGENTS block, four-field integration state, and local shared config. Tauri bundles `pi-integration/**/*`, and [[pi-lifecycle-tests#Pi Lifecycle Test Specs#Packaged Assets]] pins that package input.

### npm package

`@sharaf-nassar/quill-pi` publishes the same dependency-free `quill.ts` source that Quill bundles, with independent SemVer and support for Pi `>=0.84.0 <1` on Node.js `>=22.19.0`.

Quill repairs only its marked managed file; Pi owns package install, update, and removal. The extension no longer brokers or ranks copies, requires path opt-in, or matches an exact desktop/reporter generation before registering. Protocol 2 remains the lifecycle wire boundary, while non-empty reporter/build/capability metadata is descriptive and older protocol-2 providers remain accepted across desktop upgrades.

`.github/workflows/publish-pi-extension.yml` accepts only `pi-vX.Y.Z` tags matching package and reporter versions, requires published non-prerelease desktop `vX.Y.Z` assets first, injects that exact desktop build into the reporter source, and performs pack plus provenance publish dry runs before `npm publish --provenance --access public` from a GitHub-hosted OIDC runner.

The npm trusted-publisher record must name `sharaf-nassar/quill` and that exact workflow filename. npm's Sigstore provenance and registry attestation are the package signature; CI stores no long-lived npm token. Package metadata fixes the public registry, repository directory, MIT license, export, Pi manifest, and supported host versions. [[pi-package-tests]] pins the shipped files.

### Extension Tools and Telemetry

The single-file Pi extension is Pi's production tracking reporter and exposes Quill's local history and working-context APIs.

Install renders `context_preservation`, `activity_tracking`, and `context_telemetry` into the owned payload and deployment stamp. [[src-tauri/src/integrations/pi.rs#features_declaration]] locates the payload's `const FEATURES` declaration by its bounds rather than by exact bytes, and rendering always emits the one-line form. The payload is formatter-owned, so an exact-byte marker made install fail closed on a pure reformat and silently strand the previously deployed extension. Root context preservation registers eight plain-JSON-Schema `quill_` tools plus Pi's context router; `PI_SUBAGENT_CHILD=1` registers tracking only and exposes neither.

Persistent session start resolves lineage from one bounded parent-header read, appends compact `quill-tracking` lifecycle/direct-lineage data through Pi, then sends the same event UUID in a protocol-v2 envelope to `/api/v1/pi/track`. Pi buffers pre-assistant entries until the native session JSONL flushes. A missing session file means intentional no-session mode: no tracking entry, request, spool, or log. Notify still defers until the named transcript exists.

Every request uses the shared 1500 ms timeout. Timeout, `429`, and `503` retry once; `401` reloads config once; `409 unknown_session` reannounces the last persisted start once before one lifecycle replay. The folded sweep recovers local sessions, so no 30-second lifecycle reannounce runs. Hot handlers defer work and never await requests; only shutdown awaits its bounded lifecycle send. Process-instance identity and lifecycle sequence survive extension reload inside one Pi process.

Tracking failure never creates a second journal. Pi's own session file is the durable local lifecycle/lineage source, while `pi_session_lifecycle` and `pi_event_receipts` remain for remote-host lifecycle, ordering, idempotency, and transactional lineage that local disk folding cannot supply. Expected transport and contained protocol-delivery failures are silent unless `QUILL_DEBUG` is set. Invalid config creates no artifact and prints one notice before remaining inert.

The router ports the canonical Claude/Codex fetch and tainted-read policy to Pi's `bash`, `read`, and fetch tool inputs. It returns Pi's `{ block, reason }` result, persists at most 256 tainted paths per session, and names ready `quill_` replacements in every denial. Turning context preservation off omits the router entirely after `/reload`.

Tool requests use the main local URL for session history and the separate context origin for `/api/v1/context/*`. Both require exact loopback hostnames. History requests select the server's compact view and re-compact older oversized responses under the same 32 KiB ceiling. Every successful tool keeps its payload only in text content and returns `{ ok: true }` details so Pi does not persist a duplicate copy. Hook telemetry uses tool-execution start/end as its sole `PreToolUse`/`PostToolUse` pair, root settlement as `Stop`, and configured-child agent boundaries exclusively as `SubagentStart`/`SubagentStop`. Hook and routing requests carry lifecycle identity metadata; non-2xx and unavailable-server failures never change Pi behavior and log only under `QUILL_DEBUG`. Context telemetry remains dependent on context preservation and posts Pi routing events with all routing token estimates set to zero.

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

**Core runtime**: Tauri 2, Axum 0.8, Tokio 1, rusqlite 0.31 (bundled), Tantivy 0.25, and reqwest 0.13.

**Tauri plugins**: tauri-plugin-dialog 2, tauri-plugin-single-instance 2, tauri-plugin-window-state 2, tauri-plugin-updater 2, tauri-plugin-log 2.

**Utilities**: serde/serde_json, chrono, sha2, similar 2, regex, walkdir, dirs, nix (unix only), sentry 0.34 (default-features off, with `backtrace`/`contexts`/`panic`/`reqwest`/`rustls`) for the [[features#Crash Reporting]] backend half.

**Dev-only**: serial_test 3 — used by [[src-tauri/src/data_paths.rs]] tests to serialize global env-var mutation across the three behavioral cases for each resolver (data dir, rules dir, Claude projects dir, Codex sessions dir) so concurrent test threads don't race.

**macOS-only**: objc2-app-kit 0.3, objc2-foundation 0.3, block2 0.6 — used by [[src-tauri/src/tray_keepalive.rs]] for the workaround that rebuilds the tray after sleep/wake and screen-parameter changes.
